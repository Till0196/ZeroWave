// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映らないときの表示（E200 番台）を決める。ARIB TR-B14 第一分冊 第二部 6.12 表 6-3・表 6-4、
//! 第五分冊 第七編 8.6 表 8-5（MMT/TLV は TR-B39 表 4-4）。重なったら E202＞E201＞E203＞E204＞E210。
//! 放送中かは PAT と PMT で決め、SDT の `running_status` は見ない。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use zwcore::filters::{Kind, Program};

const SILENT_FOR: Duration = Duration::from_secs(3);
/// Mirakurun は TS が届くまで応答しないので、ふつうの選局（ロックまで数秒）より長く取る。
const UNANSWERED_FOR: Duration = Duration::from_secs(8);
const QUIET_FOR: Duration = Duration::from_secs(3);
const WORSENING_FOR: u64 = 2;
const RECOVERED_FOR: Duration = Duration::from_secs(5);
/// いちばん長い集計期間より少し長く。
const KEPT_FOR: Duration = Duration::from_secs(6);

/// 臨時映像サービスと臨時データサービス（ARIB STD-B10 表 6-50）。終わると E204。
const TEMPORARY_KINDS: [u8; 2] = [0xa1, 0xa3];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Trouble {
    Weak,
    NoSignal,
    Suspended,
    Ended,
    Unsupported,
}

impl Trouble {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Weak => "E201",
            Self::NoSignal => "E202",
            Self::Suspended => "E203",
            Self::Ended => "E204",
            Self::Unsupported => "E210",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Weak => "信号レベルが低下しています",
            Self::NoSignal => "受信できません",
            Self::Suspended => "このチャンネルは放送を休止しています",
            Self::Ended => "このチャンネルはありません",
            Self::Unsupported => "このチャンネルは受信できません",
        }
    }

    pub(crate) fn guide(self) -> &'static str {
        match self {
            Self::Weak => "アンテナ線の接続と、アンテナの向きを確認してください。",
            Self::NoSignal => "アンテナ線の接続と、チューナーの設定を確認してください。",
            Self::Suspended => "放送が始まるまでお待ちください。放送時間は番組表で確認できます。",
            Self::Ended => "臨時のサービスは終了しました。ほかのチャンネルを選んでください。",
            Self::Unsupported => {
                "このサービスの映像と音声に対応していません。ほかのチャンネルを選んでください。"
            }
        }
    }

    pub(crate) fn alarms(self) -> bool {
        matches!(self, Self::Weak | Self::NoSignal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub trouble: Trouble,
    /// E201 は映像が止まっている間だけ覆い、動いている間はステータスバーの赤字だけ。
    pub covers: bool,
    /// ログにだけ書く（画面には内部の語を出さない）。
    pub why: &'static str,
}

/// ある時点のストリームの様子。数は選局してからの通算で、増えたかだけを見る。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub bytes: u64,
    pub errors: u64,
    pub dropped: u64,
    pub others: u64,
    pub listed: Option<Vec<u16>>,
    pub mapped: bool,
    pub kind_in_network: Option<u8>,
    pub stalled: bool,
    pub unsupported: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Judge {
    service: Option<u16>,
    began: Option<Instant>,
    looks: VecDeque<(Instant, Look)>,
    refused: Option<&'static str>,
    bytes_moved: Option<Instant>,
    unmapped_since: Option<Instant>,
    worsened: Option<Instant>,
    weak: bool,
    temporary: bool,
    /// E204・E210 は別のチャンネルを選ぶまで下ろさない。
    ended: bool,
    unsupported: bool,
    shown: Option<Verdict>,
}

impl Judge {
    pub(crate) fn reset(&mut self, service: Option<u16>, now: Instant) {
        *self = Self {
            service,
            began: Some(now),
            bytes_moved: Some(now),
            ..Self::default()
        };
    }

    pub(crate) fn pause(&mut self) {
        self.shown = None;
    }

    pub(crate) fn refused(&mut self, code: &str) {
        self.refused = match code {
            "no_lock" => Some("選局の応答が no_lock（ロックしない）"),
            "no_sync" => Some("選局の応答が no_sync（同期しない）"),
            "nothing_came" => Some("選局の応答が無いまま切れた（TS が一つも届かない）"),
            _ => None,
        };
    }

    pub(crate) fn shown(&self) -> Option<Verdict> {
        self.shown
    }

    pub(crate) fn look(&mut self, now: Instant, look: Look) -> Option<Verdict> {
        let verdict = self.decide(now, look);
        self.show(verdict)
    }

    /// 選局の応答を待っている。選局を要求する前の局の様子では決めない。
    pub(crate) fn waiting(&mut self, now: Instant, asked: Instant) -> Option<Verdict> {
        let verdict = (self.service.is_some()
            && now.saturating_duration_since(asked) >= UNANSWERED_FOR)
            .then_some(Verdict {
                trouble: Trouble::NoSignal,
                covers: true,
                why: "選局の応答が 8 秒返らない（TS が届かない）",
            });
        self.show(verdict)
    }

    fn show(&mut self, verdict: Option<Verdict>) -> Option<Verdict> {
        if verdict.map(|v| (v.trouble, v.covers)) != self.shown.map(|v| (v.trouble, v.covers)) {
            match verdict {
                Some(verdict) => tracing::info!(
                    "受信エラー表示: {} {}（{}）{}",
                    verdict.trouble.code(),
                    verdict.trouble.title(),
                    verdict.why,
                    if verdict.covers {
                        ""
                    } else {
                        "　ステータスバーのみ"
                    }
                ),
                None => {
                    if let Some(was) = self.shown {
                        tracing::info!("受信エラー表示を解除（{}）", was.trouble.code());
                    }
                }
            }
        }
        self.shown = verdict;
        verdict
    }

    fn decide(&mut self, now: Instant, look: Look) -> Option<Verdict> {
        let service = self.service?;
        let began = *self.began.get_or_insert(now);
        // 数が減るのは数え直したとき。減ったら前の様子を捨てる。
        if self.looks.back().is_some_and(|(_, last)| {
            look.bytes < last.bytes
                || look.errors < last.errors
                || look.dropped < last.dropped
                || look.others < last.others
        }) {
            self.looks.clear();
        }
        let last = self.looks.back().map(|(_, last)| last.clone());
        if last.as_ref().is_none_or(|last| look.bytes > last.bytes) && look.bytes > 0 {
            self.bytes_moved = Some(now);
        }
        if last
            .as_ref()
            .is_some_and(|last| look.errors > last.errors || look.dropped > last.dropped)
        {
            self.worsened = Some(now);
        }
        if let Some(kind) = look.kind_in_network
            && TEMPORARY_KINDS.contains(&kind)
        {
            self.temporary = true;
        }
        let in_pat = look.listed.as_ref().map(|listed| listed.contains(&service));
        if in_pat == Some(true) && !look.mapped {
            self.unmapped_since.get_or_insert(now);
        } else {
            self.unmapped_since = None;
        }
        self.looks.push_back((now, look.clone()));
        while self
            .looks
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > KEPT_FOR)
        {
            self.looks.pop_front();
        }

        if let Some(why) = self.refused {
            return Some(Verdict {
                trouble: Trouble::NoSignal,
                covers: true,
                why,
            });
        }
        let silent = self
            .bytes_moved
            .is_none_or(|moved| now.duration_since(moved) >= SILENT_FOR);
        if silent {
            return Some(Verdict {
                trouble: Trouble::NoSignal,
                covers: true,
                why: "3 秒 TS が届かない",
            });
        }

        if self.rising(now) {
            self.weak = true;
        } else if self
            .worsened
            .is_none_or(|at| now.duration_since(at) >= RECOVERED_FOR)
        {
            self.weak = false;
        }
        let quiet = self
            .at_or_before(now, QUIET_FOR)
            .map(|old| look.errors == old.errors);
        let flowing = self
            .at_or_before(now, QUIET_FOR)
            .map(|old| (look.bytes > old.bytes, look.others > old.others));
        // 表 6-4 の三つの状態。(1) NIT にあるのに PAT に無い（臨時サービスは E204）。
        let missing = look.kind_in_network.is_some() && in_pat == Some(false) && !self.temporary;
        let unmapped = self
            .unmapped_since
            .is_some_and(|since| now.duration_since(since) >= QUIET_FOR);
        let nulls_only = look.listed.is_none()
            && flowing == Some((true, false))
            && now.duration_since(began) >= QUIET_FOR;
        if self.weak {
            return Some(Verdict {
                trouble: Trouble::Weak,
                covers: look.stalled || !look.mapped,
                why: "エラーかドロップが増え続けている",
            });
        }
        // エラーがあるときは休止と表示しない：(2)(3) は信号の低下として出す。
        if (unmapped || nulls_only) && quiet == Some(false) {
            return Some(Verdict {
                trouble: Trouble::Weak,
                covers: true,
                why: if unmapped {
                    "PAT にあるのに PMT が取れず、エラーがある"
                } else {
                    "ヌルパケットだけで PAT が無く、エラーがある"
                },
            });
        }
        if quiet == Some(true) {
            let why = if missing {
                Some(if look.listed.as_ref().is_some_and(Vec::is_empty) {
                    "PAT が空（サービスが全部休止）"
                } else {
                    "NIT のサービスリストにあるのに PAT に無い"
                })
            } else if unmapped {
                Some("PAT にあるのに PMT が 3 秒取れない")
            } else if nulls_only {
                Some("ヌルパケットだけで PAT が無い")
            } else {
                None
            };
            if let Some(why) = why {
                return Some(Verdict {
                    trouble: Trouble::Suspended,
                    covers: true,
                    why,
                });
            }
        }
        if self.temporary && in_pat == Some(false) {
            self.ended = true;
        }
        if self.ended {
            return Some(Verdict {
                trouble: Trouble::Ended,
                covers: true,
                why: "臨時サービスが PAT から消えた",
            });
        }
        if look.unsupported {
            self.unsupported = true;
        }
        if self.unsupported {
            return Some(Verdict {
                trouble: Trouble::Unsupported,
                covers: true,
                why: "映像も音声も使えるデコーダーが無い",
            });
        }
        None
    }

    fn rising(&self, now: Instant) -> bool {
        let Some((_, last)) = self.looks.back() else {
            return false;
        };
        let worse = |earlier: &Look, later: &Look| {
            later.errors > earlier.errors || later.dropped > earlier.dropped
        };
        let mut later = last;
        for second in 1..=WORSENING_FOR {
            let Some(earlier) = self.at_or_before(now, Duration::from_secs(second)) else {
                return false;
            };
            if !worse(earlier, later) {
                return false;
            }
            later = earlier;
        }
        true
    }

    fn at_or_before(&self, now: Instant, ago: Duration) -> Option<&Look> {
        let at = now.checked_sub(ago)?;
        self.looks
            .iter()
            .rev()
            .find(|(seen, _)| *seen <= at)
            .map(|(_, look)| look)
    }
}

/// 映像も音声も使えるデコーダーが無いなら、その映像と音声。どちらか一つでも使えれば空。
pub(crate) fn unplayable(program: &Program) -> Vec<(&'static str, String)> {
    let playable = program.streams.iter().any(|stream| match stream.kind {
        Kind::Video(codec) => zwcore::engine::can_decode(codec),
        Kind::Audio(_) => true,
        _ => false,
    });
    if playable {
        return Vec::new();
    }
    program
        .streams
        .iter()
        .filter_map(|stream| {
            let what = match stream.kind {
                Kind::Video(_) => "映像",
                Kind::Other => declared_as(&stream.declared)?,
                _ => return None,
            };
            let named = crate::ui::panes::stream::what_declared(&stream.declared);
            let name = if named == "—" {
                format!("方式 {}", stream.declared)
            } else {
                named.to_owned()
            };
            Some((what, name))
        })
        .collect()
}

fn declared_as(declared: &str) -> Option<&'static str> {
    if let Some(stream_type) = declared
        .strip_prefix("0x")
        .and_then(|digits| u8::from_str_radix(digits, 16).ok())
    {
        return match stream_type {
            // ISO/IEC 13818-1 表 2-34。
            0x01 | 0x02 | 0x10 | 0x1b | 0x1e | 0x1f | 0x20 | 0x21 | 0x24 | 0x25 | 0x33 => {
                Some("映像")
            }
            0x03 | 0x04 | 0x0f | 0x11 | 0x1c | 0x2d | 0x2e | 0x81 | 0x87 => Some("音声"),
            _ => None,
        };
    }
    match declared {
        "hev1" | "hvc1" | "avc1" | "avc3" | "vvc1" | "vvi1" => Some("映像"),
        "mp4a" | "mhm1" | "mhm2" | "alsa" => Some("音声"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fine(second: u64) -> Look {
        Look {
            bytes: 1_000_000 * (second + 1),
            others: 5_000 * (second + 1),
            listed: Some(vec![0x0400, 0x0401]),
            mapped: true,
            kind_in_network: Some(0x01),
            ..Look::default()
        }
    }

    fn run(
        judge: &mut Judge,
        from: Instant,
        seconds: u64,
        looks: impl Fn(u64) -> Look,
    ) -> Option<Verdict> {
        let mut last = None;
        for second in 0..=seconds {
            last = judge.look(from + Duration::from_secs(second), looks(second));
        }
        last
    }

    fn judge(at: Instant) -> Judge {
        let mut judge = Judge::default();
        judge.reset(Some(0x0400), at);
        judge
    }

    fn trouble(verdict: Option<Verdict>) -> Option<Trouble> {
        verdict.map(|verdict| verdict.trouble)
    }

    #[test]
    fn a_stream_that_plays_shows_nothing() {
        let at = Instant::now();
        assert_eq!(run(&mut judge(at), at, 10, fine), None);
    }

    #[test]
    fn nothing_is_judged_without_a_service() {
        let at = Instant::now();
        let mut judge = Judge::default();
        assert_eq!(run(&mut judge, at, 10, |_| Look::default()), None);
    }

    #[test]
    fn a_tuner_that_cannot_lock_says_it_at_once() {
        let at = Instant::now();
        let mut judge = judge(at);
        judge.refused("tuner_busy");
        assert_eq!(judge.look(at, Look::default()), None);
        judge.refused("no_lock");
        assert_eq!(
            trouble(judge.look(at, Look::default())),
            Some(Trouble::NoSignal)
        );
        judge.reset(Some(0x0400), at);
        assert_eq!(judge.look(at, fine(0)), None);
    }

    #[test]
    fn a_stream_cut_before_any_answer_is_no_signal() {
        let at = Instant::now();
        let mut judge = judge(at);
        judge.refused("nothing_came");
        assert_eq!(
            trouble(judge.look(at, Look::default())),
            Some(Trouble::NoSignal)
        );
    }

    #[test]
    fn an_answer_that_never_comes_is_no_signal() {
        let at = Instant::now();
        let mut judge = judge(at);
        assert_eq!(judge.waiting(at + Duration::from_secs(3), at), None);
        assert_eq!(judge.waiting(at + Duration::from_secs(7), at), None);
        assert_eq!(
            trouble(judge.waiting(at + Duration::from_secs(8), at)),
            Some(Trouble::NoSignal)
        );
        assert_eq!(judge.shown().map(|shown| shown.covers), Some(true));
        let mut idle = Judge::default();
        assert_eq!(idle.waiting(at + Duration::from_secs(20), at), None);
    }

    #[test]
    fn nothing_read_after_the_answer_is_no_signal() {
        let at = Instant::now();
        let mut judge = judge(at);
        let late = at + Duration::from_secs(15);
        assert_eq!(
            trouble(judge.look(late, Look::default())),
            Some(Trouble::NoSignal)
        );
    }

    #[test]
    fn three_silent_seconds_are_no_signal() {
        let at = Instant::now();
        let mut judge = judge(at);
        let stuck = |second: u64| fine(second.min(4));
        assert_eq!(run(&mut judge, at, 6, stuck), None);
        assert_eq!(
            trouble(judge.look(at + Duration::from_secs(8), fine(4))),
            Some(Trouble::NoSignal)
        );
        assert_eq!(judge.look(at + Duration::from_secs(9), fine(9)), None);
    }

    #[test]
    fn a_service_the_pat_left_out_is_suspended() {
        let at = Instant::now();
        let mut judge = judge(at);
        let missing = |second: u64| Look {
            listed: Some(vec![0x0401]),
            mapped: false,
            ..fine(second)
        };
        assert_eq!(judge.look(at, missing(0)), None, "3 秒見るまでは判定しない");
        assert_eq!(
            trouble(run(&mut judge, at, 4, missing)),
            Some(Trouble::Suspended)
        );
        assert_eq!(judge.look(at + Duration::from_secs(5), fine(5)), None);
    }

    #[test]
    fn an_empty_pat_suspends_everything() {
        let at = Instant::now();
        let verdict = run(&mut judge(at), at, 4, |second| Look {
            listed: Some(Vec::new()),
            mapped: false,
            ..fine(second)
        });
        assert_eq!(trouble(verdict), Some(Trouble::Suspended));
    }

    #[test]
    fn a_map_that_never_comes_is_suspended() {
        let at = Instant::now();
        let mut judge = judge(at);
        let unmapped = |second: u64| Look {
            mapped: false,
            ..fine(second)
        };
        assert_eq!(run(&mut judge, at, 2, unmapped), None);
        assert_eq!(
            trouble(run(&mut judge, at, 4, unmapped)),
            Some(Trouble::Suspended)
        );
    }

    #[test]
    fn nothing_but_null_packets_is_suspended() {
        let at = Instant::now();
        let verdict = run(&mut judge(at), at, 4, |second| Look {
            bytes: 1_000_000 * (second + 1),
            ..Look::default()
        });
        assert_eq!(trouble(verdict), Some(Trouble::Suspended));
    }

    #[test]
    fn errors_are_never_called_a_suspension() {
        let at = Instant::now();
        let verdict = run(&mut judge(at), at, 4, |second| Look {
            mapped: false,
            errors: if second >= 3 { 1 } else { 0 },
            ..fine(second)
        });
        assert_eq!(trouble(verdict), Some(Trouble::Weak));
        let at = Instant::now();
        let verdict = run(&mut judge(at), at, 4, |second| Look {
            listed: Some(vec![0x0401]),
            mapped: false,
            errors: if second >= 3 { 1 } else { 0 },
            ..fine(second)
        });
        assert_eq!(verdict, None);
    }

    #[test]
    fn errors_that_keep_rising_are_a_weak_signal() {
        let at = Instant::now();
        let mut judge = judge(at);
        let rising = |second: u64| Look {
            errors: second * 10,
            ..fine(second)
        };
        let verdict = run(&mut judge, at, 3, rising).expect("E201");
        assert_eq!(verdict.trouble, Trouble::Weak);
        assert!(!verdict.covers);
        assert!(verdict.trouble.alarms());
        let stalled = judge
            .look(
                at + Duration::from_secs(4),
                Look {
                    stalled: true,
                    ..rising(4)
                },
            )
            .expect("E201");
        assert!(stalled.covers);
        let calm = |second: u64| Look {
            errors: 40,
            ..fine(second)
        };
        for second in 5..9 {
            assert_eq!(
                trouble(judge.look(at + Duration::from_secs(second), calm(second))),
                Some(Trouble::Weak),
                "{second}"
            );
        }
        assert_eq!(judge.look(at + Duration::from_secs(9), calm(9)), None);
    }

    #[test]
    fn one_drop_is_not_a_weak_signal() {
        let at = Instant::now();
        let verdict = run(&mut judge(at), at, 6, |second| Look {
            dropped: if second >= 3 { 1 } else { 0 },
            ..fine(second)
        });
        assert_eq!(verdict, None);
    }

    #[test]
    fn a_temporary_service_that_ended_is_gone() {
        let at = Instant::now();
        let mut judge = judge(at);
        let temporary = |second: u64| Look {
            kind_in_network: Some(0xa1),
            ..fine(second)
        };
        assert_eq!(run(&mut judge, at, 2, temporary), None);
        let gone = |second: u64| Look {
            listed: Some(vec![0x0401]),
            mapped: false,
            kind_in_network: None,
            ..fine(second)
        };
        let later = at + Duration::from_secs(3);
        assert_eq!(
            trouble(run(&mut judge, later, 5, gone)),
            Some(Trouble::Ended)
        );
        assert_eq!(
            trouble(judge.look(at + Duration::from_secs(9), fine(9))),
            Some(Trouble::Ended)
        );
        judge.reset(Some(0x0401), at + Duration::from_secs(10));
        assert_eq!(judge.look(at + Duration::from_secs(10), fine(10)), None);
    }

    #[test]
    fn a_service_nothing_can_decode_is_unsupported() {
        let at = Instant::now();
        let verdict = run(&mut judge(at), at, 1, |second| Look {
            unsupported: true,
            ..fine(second)
        });
        assert_eq!(trouble(verdict), Some(Trouble::Unsupported));
    }

    #[test]
    fn the_troubles_are_ranked() {
        let at = Instant::now();
        let verdict = run(&mut judge(at), at, 3, |second| Look {
            errors: second * 10,
            unsupported: true,
            ..fine(second)
        });
        assert_eq!(trouble(verdict), Some(Trouble::Weak));
        let verdict = run(&mut judge(at), at, 4, |second| Look {
            mapped: false,
            unsupported: true,
            ..fine(second)
        });
        assert_eq!(trouble(verdict), Some(Trouble::Suspended));
        let mut judge = judge(at);
        judge.refused("no_sync");
        let verdict = run(&mut judge, at, 4, |second| Look {
            errors: second * 10,
            mapped: false,
            unsupported: true,
            ..fine(second)
        });
        assert_eq!(trouble(verdict), Some(Trouble::NoSignal));
    }

    fn stream(kind: Kind, declared: &str) -> zwcore::filters::Stream {
        zwcore::filters::Stream {
            id: 0x0100,
            kind,
            framing: mediadecoder::es::Framing::StartCodes,
            declared: declared.to_owned(),
            tag: None,
            component: None,
            name: None,
            hierarchy: None,
            data: None,
            decode: None,
        }
    }

    fn program(streams: Vec<zwcore::filters::Stream>) -> Program {
        Program {
            number: 0x0400,
            map: 0x01f0,
            streams,
            pcr: 0x1fff,
            ecm: Vec::new(),
            application: None,
            region: None,
            emergency_news: None,
            emergency: Vec::new(),
        }
    }

    #[test]
    fn unplayable_needs_neither_picture_nor_sound() {
        let neither = program(vec![
            stream(Kind::Other, "0x10"),
            stream(Kind::Other, "0x1c"),
            stream(Kind::Data, "0x0d"),
        ]);
        assert_eq!(
            unplayable(&neither),
            vec![
                ("映像", String::from("方式 0x10")),
                ("音声", String::from("方式 0x1c"))
            ]
        );
        let sound = program(vec![
            stream(Kind::Other, "0x10"),
            stream(Kind::Audio(zwcore::filters::Audio::Adts), "0x0f"),
        ]);
        assert!(unplayable(&sound).is_empty());
        assert!(unplayable(&program(vec![stream(Kind::Data, "0x0d")])).is_empty());
    }
}
