// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 統計。五秒ごとのログと、スレッドごとの時間の内訳。数えるのは各スレッド。

use super::*;

pub(super) const METER: u64 = 5;

/// 選局のログを、映像か音声が揃わないまま出すまでの時間（映像や音声の無い局もある）。
pub(super) const FIRST_MOST: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
pub(super) struct First {
    pub(super) access: Option<Duration>,
    pub(super) picture: Option<Duration>,
    pub(super) sound: Option<Duration>,
    pub(super) said: bool,
}

/// Where a run of turns went. Timing the stages apart is the only way to tell
/// deciding not to draw from waiting for the display from being slow.
#[derive(Debug)]
pub(super) struct Meter {
    pub(super) began: Instant,
    pub(super) turns: u64,
    pub(super) feed: Duration,
    pub(super) tick: Duration,
    pub(super) drawn: u64,
    pub(super) late: u64,
    pub(super) redrawn: u64,
    /// The wall clock between one draw and the next (ms): the cadence the viewer sees.
    pub(super) gaps: Vec<f32>,
    pub(super) lags: Vec<f32>,
    pub(super) missed: u64,
    pub(super) dropped: u64,
    pub(super) unseen: u64,
    pub(super) source: u64,
    pub(super) pacing: u64,
    pub(super) apart: u64,
    pub(super) onward: u64,
    pub(super) fetching: u64,
    pub(super) pushing: u64,
    pub(super) blocks: u64,
    pub(super) waiting: u64,
    pub(super) decoding: u64,
    pub(super) handing: u64,
    pub(super) bytes: u64,
    pub(super) heard: u64,
}

impl Meter {
    pub(super) fn new() -> Self {
        Self {
            began: Instant::now(),
            turns: 0,
            feed: Duration::ZERO,
            tick: Duration::ZERO,
            drawn: 0,
            late: 0,
            redrawn: 0,
            gaps: Vec::new(),
            lags: Vec::new(),
            missed: 0,
            dropped: 0,
            unseen: 0,
            source: 0,
            pacing: 0,
            apart: 0,
            onward: 0,
            fetching: 0,
            pushing: 0,
            blocks: 0,
            waiting: 0,
            decoding: 0,
            handing: 0,
            bytes: 0,
            heard: 0,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Reading {
    pub(super) source: AtomicU64,
    pub(super) pacing: AtomicU64,
    pub(super) apart: AtomicU64,
    pub(super) onward: AtomicU64,
    pub(super) missed: AtomicU64,
    /// 一回の最長、マイクロ秒：ソース待ち、プラグイン、分離、受け渡し。
    pub(super) longest: [AtomicU64; 4],
}

impl Reading {
    pub(super) fn longest(&self) -> [u64; 4] {
        std::array::from_fn(|at| self.longest[at].load(Ordering::Relaxed))
    }

    pub(super) fn forget_longest(&self) {
        for longest in &self.longest {
            longest.store(0, Ordering::Relaxed);
        }
    }
}

/// フェッチスレッドの時間の内訳、マイクロ秒。
#[derive(Debug, Default)]
pub(super) struct Filling {
    pub(super) waiting: AtomicU64,
    /// 録画では、いちばん遅いリーダーを待つ時間でもある。
    pub(super) pushing: AtomicU64,
    pub(super) blocks: AtomicU64,
}

#[derive(Debug, Default)]
pub(super) struct Decodes {
    pub(super) waiting: AtomicU64,
    pub(super) decoding: AtomicU64,
    pub(super) onward: AtomicU64,
    pub(super) frames: [AtomicU64; 3],
    pub(super) repeats: AtomicU64,
    /// フィールドピクチャーは一つのフィールドで一枚と数える。
    pub(super) structures: [AtomicU64; 2],
    pub(super) troubles: AtomicU64,
    pub(super) about: std::sync::Mutex<Decoder>,
}

#[derive(Debug, Clone, Default)]
pub struct Decoder {
    pub coded: Option<(mediadecoder::es::Codec, mediadecoder::es::Format)>,
    pub decoder: Option<(String, bool)>,
    /// 真なら ZeroWave のデインターレースと逆テレシネは掛けない。
    pub deinterlaced: bool,
}

pub(super) fn spread(values: &mut [f32]) -> (f32, f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    (
        values[0],
        values[values.len() / 2],
        values[values.len() - 1],
    )
}

impl Player {
    pub(super) fn watch_first(&mut self) {
        if self.first.said {
            return;
        }
        let since = self.tuned.elapsed();
        if self.first.access.is_none() && self.newest_access.load(Ordering::Relaxed) != 0 {
            self.first.access = Some(since);
        }
        if self.first.picture.is_none() && self.drawn > 0 {
            self.first.picture = Some(since);
        }
        if self.first.sound.is_none() && self.shared.heard.load(Ordering::Relaxed) != 0 {
            self.first.sound = Some(since);
        }
        let both = self.first.picture.is_some() && self.first.sound.is_some();
        if !both && since < FIRST_MOST {
            return;
        }
        self.first.said = true;
        let said = |what: Option<Duration>| match what {
            Some(took) => format!("{:.0} ms", took.as_secs_f64() * 1000.0),
            None => String::from("未着"),
        };
        tracing::info!(
            "選局から  データ {}  映像 {}  音声 {}",
            said(self.first.access),
            said(self.first.picture),
            said(self.first.sound),
        );
        crate::memory::give_back();
    }

    pub(super) fn measure(&mut self) {
        let over = self.meter.began.elapsed();
        if over.as_secs() < METER {
            return;
        }
        let seconds = over.as_secs_f64();
        let Spent {
            upload,
            acquire,
            draw,
            show,
            pushed,
            shown: _,
            woven,
            locked,
            whole_frames: _,
            soft: _,
            matched,
            film,
            overlay,
            film_changes,
            combed,
            stripes: _,
        } = {
            self.drain_spent();
            std::mem::take(&mut self.spent_meter)
        };
        let heard = self.shared.heard.load(Ordering::Relaxed);
        let missed = self.shared.missed.load(Ordering::Relaxed);
        let dropped = self.shared.dropped.load(Ordering::Relaxed);
        let repeated = self.shared.repeated.load(Ordering::Relaxed);
        let step = self.shared.with_clock(|clock| clock.step());
        let ms = |spent: Duration| spent.as_secs_f64() * 1000.0 / seconds;
        let lock = if locked {
            "ロック中"
        } else {
            "未ロック"
        };
        let matching = if matched > 0 || film || overlay || film_changes > 0 || combed > 0 {
            format!(
                "、フィールドマッチ {matched} 枚（{}、コーミング {combed}{}）",
                if film {
                    "film"
                } else if overlay {
                    "60i: film に動くテロップ"
                } else {
                    "60i"
                },
                if film_changes > 0 {
                    format!("、切り替え {film_changes} 回")
                } else {
                    String::new()
                }
            )
        } else {
            String::new()
        };
        // 一つのログにまとめる（行ごとに出すと、ヘッダーが行ごとに付く）。
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!(
            "{seconds:.1}秒: 表示 {} 枚 ({:.1}/秒)、GPU へ {pushed} 枚、\
             遅延で破棄 {}、再描画 {}、ループ {}  \
             1秒あたり ms — 受け取り {:.0}、転送 {:.0}、サーフェス待ち {:.0}、レンダリング {:.0}、表示 {:.0}、他 {:.0}  \
             現在 デコード済み {}、表示待ち {}、GPU {}、音声 {}、音声の間隔 {:.1} ms、\
             音声の欠落 {:.0} ms、破棄したデータ {}、非表示中に破棄 {}、周期で間引き {woven} 枚（{lock}）{matching}",
            self.drawn - self.meter.drawn,
            (self.drawn - self.meter.drawn) as f64 / seconds,
            self.late - self.meter.late,
            self.redrawn - self.meter.redrawn,
            self.meter.turns,
            ms(self.meter.feed),
            ms(upload),
            ms(acquire),
            ms(draw),
            ms(show),
            ms(self.meter.tick.saturating_sub(acquire + draw + show)),
            self.ready.len(),
            self.due.len(),
            self.video.as_ref().map_or(0, Screen::pending),
            self.shared.queued.load(Ordering::Relaxed),
            step as f64 * 1000.0 / f64::from(self.rate.max(1)),
            missed.saturating_sub(self.meter.missed) as f64 * 1000.0 / f64::from(self.rate.max(1)),
            self.dropped_access
                .load(Ordering::Relaxed)
                .saturating_sub(self.meter.dropped),
            self.unseen - self.meter.unseen,
        ));
        {
            // 選局のたびに数え直すので、前回の値のほうが大きいことがある。
            let us = |value: u64, was: u64| value.saturating_sub(was) as f64 / 1000.0 / seconds;
            let bytes = self.read_bytes.load(Ordering::Relaxed);
            lines.push(format!(
                "         ソース {:.2} Mbit/秒、音声 {:.0} サンプル/秒（欠落 {:.0}/秒、計 {:.0}）",
                bytes.saturating_sub(self.meter.bytes) as f64 * 8.0 / 1e6 / seconds,
                heard.saturating_sub(self.meter.heard) as f64 / seconds,
                missed.saturating_sub(self.meter.missed) as f64 / seconds,
                (heard.saturating_sub(self.meter.heard) + missed.saturating_sub(self.meter.missed))
                    as f64
                    / seconds,
            ));
            let newest = self.newest_access.load(Ordering::Relaxed) as i64;
            if let Some(now) = self.now()
                && newest > 0
            {
                lines.push(format!(
                    "         表示の遅れ {:.0} ms（ソース比）",
                    (newest - now) as f64 / 90.0,
                ));
            }
            let oldest = self.oldest_access.load(Ordering::Relaxed) as i64;
            if oldest > 0 && newest >= oldest && self.queued_access.load(Ordering::Relaxed) > 0 {
                lines.push(format!(
                    "         未デコードのデータ {:.0} ms（上限 {:.0}）",
                    (newest - oldest) as f64 / 90.0,
                    SPAN_MOST as f64 / 90.0,
                ));
            }
            let source = self.spent_reading.source.load(Ordering::Relaxed);
            let pacing = self.spent_reading.pacing.load(Ordering::Relaxed);
            let apart = self.spent_reading.apart.load(Ordering::Relaxed);
            let onward = self.spent_reading.onward.load(Ordering::Relaxed);
            lines.push(format!(
                "         読み取りスレッド 1秒あたり ms — ソース待ち {:.0}、ペース調整 {:.0}、分離 {:.0}、受け渡し {:.0}",
                us(source, self.meter.source),
                us(pacing, self.meter.pacing),
                us(apart, self.meter.apart),
                us(onward, self.meter.onward),
            ));
            // `ソース待ち` が一秒に近ければ、遅いのはソース。
            let fetching = self.spent_filling.waiting.load(Ordering::Relaxed);
            let pushing = self.spent_filling.pushing.load(Ordering::Relaxed);
            let blocks = self.spent_filling.blocks.load(Ordering::Relaxed);
            let heaped = blocks.saturating_sub(self.meter.blocks);
            lines.push(format!(
                "         フェッチスレッド 1秒あたり ms — ソース待ち {:.0}、リングバッファ書き込み {:.0}  \
                 ブロック {:.0}/秒（平均 {} バイト）、リングバッファ {} KiB",
                us(fetching, self.meter.fetching),
                us(pushing, self.meter.pushing),
                heaped as f64 / seconds,
                bytes
                    .saturating_sub(self.meter.bytes)
                    .checked_div(heaped)
                    .unwrap_or(0),
                self.ring.as_ref().map_or(0, |ring| ring.held() / 1024),
            ));
            let waiting = self.spent_decoding.waiting.load(Ordering::Relaxed);
            let decoding = self.spent_decoding.decoding.load(Ordering::Relaxed);
            let handing = self.spent_decoding.onward.load(Ordering::Relaxed);
            lines.push(format!(
                "         デコードスレッド 1秒あたり ms — データ待ち {:.0}、デコード {:.0}、受け渡し {:.0}",
                us(waiting, self.meter.waiting),
                us(decoding, self.meter.decoding),
                us(handing, self.meter.handing),
            ));
        }
        {
            let short = self.meter.gaps.iter().filter(|gap| **gap < 13.0).count();
            let long = self.meter.gaps.iter().filter(|gap| **gap > 20.0).count();
            let (least, middle, most) = spread(&mut self.meter.gaps);
            let (lag_least, lag_middle, lag_most) = spread(&mut self.meter.lags);
            lines.push(format!(
                "         表示間隔 ms 最小 {least:.1} 中央 {middle:.1} 最大 {most:.1}、\
                 13 未満 {short}、20 超 {long}  \
                 表示時刻からの遅れ ms 最小 {lag_least:.1} 中央 {lag_middle:.1} 最大 {lag_most:.1}"
            ));
        }
        if dropped > 0 || repeated > 0 {
            lines.push(format!(
                "         パススルー: 破棄 {dropped}、重複 {repeated}"
            ));
        }
        tracing::debug!("{}", lines.join("\n"));
        self.meter = Meter::new();
        self.meter.drawn = self.drawn;
        self.meter.late = self.late;
        self.meter.redrawn = self.redrawn;
        self.meter.missed = missed;
        self.meter.dropped = self.dropped_access.load(Ordering::Relaxed);
        self.meter.unseen = self.unseen;
        self.meter.source = self.spent_reading.source.load(Ordering::Relaxed);
        self.meter.pacing = self.spent_reading.pacing.load(Ordering::Relaxed);
        self.meter.apart = self.spent_reading.apart.load(Ordering::Relaxed);
        self.meter.onward = self.spent_reading.onward.load(Ordering::Relaxed);
        self.meter.fetching = self.spent_filling.waiting.load(Ordering::Relaxed);
        self.meter.pushing = self.spent_filling.pushing.load(Ordering::Relaxed);
        self.meter.blocks = self.spent_filling.blocks.load(Ordering::Relaxed);
        self.meter.waiting = self.spent_decoding.waiting.load(Ordering::Relaxed);
        self.meter.decoding = self.spent_decoding.decoding.load(Ordering::Relaxed);
        self.meter.handing = self.spent_decoding.onward.load(Ordering::Relaxed);
        self.meter.bytes = self.read_bytes.load(Ordering::Relaxed);
        self.meter.heard = heard;
    }

    pub fn report(&self) {
        tracing::info!(
            "表示 {} 枚、遅延で破棄 {} 枚、音声 {} サンプル",
            self.drawn,
            self.late,
            self.shared.heard.load(Ordering::Relaxed)
        );
        let dropped = self.shared.dropped.load(Ordering::Relaxed);
        let repeated = self.shared.repeated.load(Ordering::Relaxed);
        if dropped > 0 || repeated > 0 {
            tracing::debug!(
                "パススルー: 破棄 {dropped} フレーム、重複 {repeated} フレーム（1フレーム 21.33 ms）"
            );
        }
    }
}
