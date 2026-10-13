// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ウィンドウが隠れてから戻るまで。戻ってから最初のフレームが出るまでと、その後しばらくの様子をそれぞれ一行のログに出す。

use super::*;

pub(super) const BACK_WATCH: Duration = Duration::from_secs(10);

/// これより短く隠れてすぐフレームが出たならログに出さない（サーフェスが一瞬隠れるのは珍しくない）。
const QUIET_HIDDEN: Duration = Duration::from_secs(1);
const QUIET_FIRST: Duration = Duration::from_millis(500);

/// 時刻は呼び出し側が渡す（テストで時間の経過を再現するため）。
#[derive(Debug, Default)]
pub(super) struct Away {
    hidden_at: Option<Instant>,
    back: Option<Back>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Counts {
    pub(super) drawn: u64,
    pub(super) late: u64,
    pub(super) dropped: u64,
    pub(super) missed_blocks: u64,
    pub(super) holes: u64,
    pub(super) left: u64,
    /// 戻ったときに 0 から測り直すので、差ではなくそのまま出す。
    pub(super) longest: [u64; 4],
}

#[derive(Debug)]
struct Back {
    at: Instant,
    hidden_for: Duration,
    took: Counts,
    thrown_due: usize,
    thrown_ready: usize,
    first_turn: Option<Duration>,
    first_picture: Option<Duration>,
    idles: u32,
    timeouts: u32,
    last_call: Instant,
    longest_gap: Duration,
    said_first: bool,
}

impl Away {
    pub(super) fn hide(&mut self, now: Instant) {
        if self.hidden_at.is_none() {
            self.hidden_at = Some(now);
        }
    }

    #[cfg(test)]
    fn hidden(&self) -> bool {
        self.hidden_at.is_some()
    }

    /// 戻った。隠れていたなら真。
    pub(super) fn show(&mut self, now: Instant, took: Counts) -> bool {
        let Some(since) = self.hidden_at.take() else {
            return false;
        };
        self.back = Some(Back {
            at: now,
            hidden_for: now.saturating_duration_since(since),
            took,
            thrown_due: 0,
            thrown_ready: 0,
            first_turn: None,
            first_picture: None,
            idles: 0,
            timeouts: 0,
            last_call: now,
            longest_gap: Duration::ZERO,
            said_first: false,
        });
        true
    }

    pub(super) fn threw(&mut self, due: usize, ready: usize) {
        if let Some(back) = self.back.as_mut() {
            back.thrown_due += due;
            back.thrown_ready += ready;
        }
    }

    /// ループを一回回った。`turn` が偽ならレンダリングしなかった回。
    pub(super) fn called(&mut self, now: Instant, turn: bool) {
        if self.hidden_at.is_some() {
            return;
        }
        let Some(back) = self.back.as_mut() else {
            return;
        };
        back.longest_gap = back
            .longest_gap
            .max(now.saturating_duration_since(back.last_call));
        back.last_call = now;
        if turn {
            back.first_turn
                .get_or_insert(now.saturating_duration_since(back.at));
        } else if back.first_turn.is_none() {
            back.idles += 1;
        }
    }

    pub(super) fn timed_out(&mut self) {
        if let Some(back) = self.back.as_mut()
            && back.first_turn.is_none()
        {
            back.timeouts += 1;
        }
    }

    pub(super) fn drew(&mut self, now: Instant) {
        if self.hidden_at.is_some() {
            return;
        }
        if let Some(back) = self.back.as_mut() {
            back.first_picture
                .get_or_insert(now.saturating_duration_since(back.at));
        }
    }

    pub(super) fn report(&mut self, now: Instant, counts: Counts) -> Option<String> {
        let back = self.back.as_mut()?;
        let since = now.saturating_duration_since(back.at);
        let watched = since >= BACK_WATCH;
        if back.hidden_for < QUIET_HIDDEN
            && back.first_picture.is_some_and(|first| first <= QUIET_FIRST)
        {
            self.back = None;
            return None;
        }
        let mut lines = Vec::new();
        if !back.said_first && (back.first_picture.is_some() || watched) {
            back.said_first = true;
            lines.push(back.first_line());
        }
        if watched {
            lines.push(back.second_line(counts));
            self.back = None;
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

fn ms(spent: Option<Duration>) -> String {
    match spent {
        Some(spent) => format!("{:.0} ms", spent.as_secs_f64() * 1000.0),
        None => String::from("まだ"),
    }
}

impl Back {
    fn first_line(&self) -> String {
        format!(
            "ウィンドウが戻った（{:.1} 秒隠れていた）: 最初のフレームまで {}（レンダリングする回まで {}、その前のレンダリングしなかった回 {}、\
             サーフェス取得のタイムアウト {}、ループ間隔の最長 {}）、戻って捨てた 表示待ち {}・デコード済みのフレーム {}",
            self.hidden_for.as_secs_f64(),
            ms(self.first_picture),
            ms(self.first_turn),
            self.idles,
            self.timeouts,
            ms(Some(self.longest_gap)),
            self.thrown_due,
            self.thrown_ready,
        )
    }

    fn second_line(&self, counts: Counts) -> String {
        let took = self.took;
        format!(
            "戻ってから {} 秒: レンダリング {} 枚、遅れて捨てた {}、読み飛ばし {} 回、捨てた符号化データ {}、\
             リングバッファから落ちたブロック {}、音声の欠落 {} サンプル  \
             読み取りスレッドの一回の最長 ms — ソース待ち {:.0}、プラグイン {:.0}、分離 {:.0}、渡す {:.0}",
            BACK_WATCH.as_secs(),
            counts.drawn.saturating_sub(took.drawn),
            counts.late.saturating_sub(took.late),
            counts.left.saturating_sub(took.left),
            counts.dropped.saturating_sub(took.dropped),
            counts.missed_blocks.saturating_sub(took.missed_blocks),
            counts.holes.saturating_sub(took.holes),
            counts.longest[0] as f64 / 1000.0,
            counts.longest[1] as f64 / 1000.0,
            counts.longest[2] as f64 / 1000.0,
            counts.longest[3] as f64 / 1000.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_return_says_how_long_until_the_first_picture() {
        let start = Instant::now();
        let mut away = Away::default();
        away.hide(at(start, 0));
        away.hide(at(start, 60_000));
        away.called(at(start, 60_000), false);
        assert!(away.hidden());
        assert!(away.show(at(start, 120_000), Counts::default()));
        assert!(!away.hidden());
        away.threw(3, 2);
        for second in 1..=4 {
            away.timed_out();
            away.called(at(start, 120_000 + second * 1000), false);
        }
        assert_eq!(away.report(at(start, 124_000), Counts::default()), None);
        away.called(at(start, 124_016), true);
        away.drew(at(start, 124_016));
        let first = away
            .report(at(start, 124_020), Counts::default())
            .expect("最初のフレームで一行目");
        assert!(first.contains("120.0 秒隠れていた"), "{first}");
        assert!(first.contains("最初のフレームまで 4016 ms"), "{first}");
        assert!(first.contains("レンダリングしなかった回 4"), "{first}");
        assert!(first.contains("サーフェス取得のタイムアウト 4"), "{first}");
        assert!(first.contains("ループ間隔の最長 1000 ms"), "{first}");
        assert!(
            first.contains("表示待ち 3・デコード済みのフレーム 2"),
            "{first}"
        );
        assert_eq!(away.report(at(start, 125_000), Counts::default()), None);
        let counts = Counts {
            drawn: 280,
            late: 4,
            dropped: 30,
            missed_blocks: 0,
            holes: 0,
            left: 1,
            longest: [3_000, 1_000, 2_000, 7_000_000],
        };
        let second = away
            .report(at(start, 130_000), counts)
            .expect("十秒で二行目");
        assert!(second.contains("レンダリング 280 枚"), "{second}");
        assert!(second.contains("読み飛ばし 1 回"), "{second}");
        assert!(second.contains("捨てた符号化データ 30"), "{second}");
        assert!(second.contains("渡す 7000"), "{second}");
        assert!(!second.contains("ウィンドウが戻った"), "{second}");
        assert_eq!(
            away.report(at(start, 140_000), counts),
            None,
            "もう出さない"
        );
    }

    #[test]
    fn a_stalled_loop_is_told_apart_and_said_even_without_a_picture() {
        let start = Instant::now();
        let mut away = Away::default();
        away.hide(at(start, 0));
        let took = Counts {
            drawn: 100,
            missed_blocks: 5,
            ..Counts::default()
        };
        assert!(away.show(at(start, 5_000), took));
        away.called(at(start, 9_000), true);
        let counts = Counts {
            drawn: 100,
            missed_blocks: 805,
            ..Counts::default()
        };
        let said = away.report(at(start, 15_000), counts).expect("十秒で出す");
        assert!(said.contains("最初のフレームまで まだ"), "{said}");
        assert!(said.contains("レンダリングする回まで 4000 ms"), "{said}");
        assert!(said.contains("ループ間隔の最長 4000 ms"), "{said}");
        assert!(said.contains("レンダリング 0 枚"), "{said}");
        assert!(
            said.contains("リングバッファから落ちたブロック 800"),
            "{said}"
        );
    }

    #[test]
    fn showing_without_hiding_is_not_a_return() {
        let start = Instant::now();
        let mut away = Away::default();
        assert!(!away.show(start, Counts::default()));
        away.called(at(start, 10), true);
        away.drew(at(start, 10));
        assert_eq!(away.report(at(start, 20_000), Counts::default()), None);
    }

    #[test]
    fn a_blink_is_not_said() {
        let start = Instant::now();
        let mut away = Away::default();
        away.hide(start);
        assert!(away.show(at(start, 300), Counts::default()));
        away.called(at(start, 320), true);
        away.drew(at(start, 320));
        assert_eq!(away.report(at(start, 330), Counts::default()), None);
        assert_eq!(away.report(at(start, 20_000), Counts::default()), None);
    }

    #[test]
    fn what_happens_while_hidden_is_not_counted() {
        let start = Instant::now();
        let mut away = Away::default();
        away.hide(start);
        away.called(at(start, 100), true);
        away.drew(at(start, 100));
        assert!(away.show(at(start, 1_000), Counts::default()));
        away.called(at(start, 1_010), true);
        away.drew(at(start, 1_010));
        let said = away
            .report(at(start, 1_020), Counts::default())
            .expect("一行目");
        assert!(said.contains("最初のフレームまで 10 ms"), "{said}");
        assert!(said.contains("1.0 秒隠れていた"), "{said}");
    }
}
