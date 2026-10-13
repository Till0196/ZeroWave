// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! どのスロットのフレームを、どの順で、何を参照してレンダリングするか（wgpu と GLES 2.0 で共用）。
//! 時間方向のフィルターが後ろのフィールドを読むので、出力は 1 フレーム遅れる。

use std::collections::VecDeque;

use super::field::{Output, Plan, Source};

#[derive(Debug)]
struct Held {
    slot: usize,
    plan: Plan,
}

/// ストリームの端では隣が自分自身になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ready {
    pub output: Output,
    pub before: usize,
    pub cur: usize,
    pub after: usize,
    /// 前後のフレームのスロット（`frames[2]` が自分）。届かない所はいちばん近いフレームで埋める。
    pub frames: [usize; 5],
}

#[derive(Debug)]
pub struct Schedule {
    slots: usize,
    pending: VecDeque<Held>,
    outputs: VecDeque<Ready>,
    /// 次のフレームの「前のフィールド」なので、出力が無くても解放できない。
    previous: Option<usize>,
    earlier: Option<usize>,
    /// レンダリングし直す（一時停止中に画面だけ変わった）ために空けない。
    shown: Option<Ready>,
    /// 1 か 2。
    behind: usize,
    ahead: usize,
}

impl Schedule {
    pub fn new(slots: usize) -> Self {
        Self::with_reach(slots, 1, 1)
    }

    pub fn with_reach(slots: usize, behind: usize, ahead: usize) -> Self {
        Self {
            slots,
            pending: VecDeque::new(),
            outputs: VecDeque::new(),
            previous: None,
            earlier: None,
            shown: None,
            behind: behind.clamp(1, 2),
            ahead: ahead.clamp(1, 2),
        }
    }

    pub fn reach(&self) -> (usize, usize) {
        (self.behind, self.ahead)
    }

    pub fn free(&self) -> Option<usize> {
        (0..self.slots).find(|slot| self.is_free(*slot))
    }

    pub fn is_free(&self, slot: usize) -> bool {
        Some(slot) != self.previous
            && Some(slot) != self.earlier
            && self.shown.is_none_or(|shown| shown.cur != slot)
            && !self.pending.iter().any(|held| held.slot == slot)
            && !self.outputs.iter().any(|ready| {
                [ready.before, ready.cur, ready.after].contains(&slot)
                    || ready.frames.contains(&slot)
            })
    }

    pub fn hold(&mut self, slot: usize, plan: Plan) {
        self.pending.push_back(Held { slot, plan });
        self.expand(false);
    }

    /// `ending` なら最後の 1 フレームも展開する。
    pub fn expand(&mut self, ending: bool) {
        while self.pending.len() > self.ahead || (ending && !self.pending.is_empty()) {
            let held = self.pending.pop_front().expect("just checked");
            let after = self.pending.front().map_or(held.slot, |next| next.slot);
            let before = self.previous.unwrap_or(held.slot);
            let later = self.pending.get(1).map_or(after, |next| next.slot);
            let earlier = self.earlier.unwrap_or(before);
            let frames = [earlier, before, held.slot, after, later];
            for output in held.plan {
                let pick = |source: Source| match source {
                    Source::Previous => before,
                    Source::Current => held.slot,
                    Source::Next => after,
                };
                self.outputs.push_back(Ready {
                    output,
                    before: pick(output.before),
                    cur: held.slot,
                    after: pick(output.after),
                    frames,
                });
            }
            if self.behind > 1 {
                self.earlier = self.previous;
            }
            self.previous = Some(held.slot);
        }
    }

    pub fn front(&self) -> Option<Ready> {
        self.outputs.front().copied()
    }

    pub fn pop(&mut self) -> bool {
        self.outputs.pop_front().is_some()
    }

    /// そのフレームは次に出すまで空けない（レンダリングし直せるように）。
    pub fn pop_shown(&mut self) -> Option<Ready> {
        let ready = self.outputs.pop_front()?;
        self.shown = Some(ready);
        Some(ready)
    }

    pub fn shown(&self) -> Option<Ready> {
        self.shown
    }

    pub fn len(&self) -> usize {
        self.outputs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty()
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.outputs.clear();
        self.previous = None;
        self.earlier = None;
        self.shown = None;
    }

    /// 保留中のフレームは残す。
    pub fn forget_outputs(&mut self) -> usize {
        let forgotten = self.outputs.len();
        self.outputs.clear();
        forgotten
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::video::field::Parity;

    #[test]
    fn the_frame_last_shown_is_kept_until_the_next_is_shown() {
        let weave = || vec![output(0, 1, Source::Current, Source::Current)];
        let mut schedule = Schedule::new(4);
        for slot in [1, 2, 0, 3] {
            schedule.hold(slot, weave());
        }
        // 出せるのは 1・2・0。3 は後ろの待ちで、0 は直前に展開したフレーム。
        assert_eq!(schedule.pop_shown().map(|ready| ready.cur), Some(1));
        assert_eq!(schedule.free(), None);
        assert_eq!(schedule.pop_shown().map(|ready| ready.cur), Some(2));
        assert_eq!(schedule.free(), Some(1));
    }

    fn output(at: u8, of: u8, before: Source, after: Source) -> Output {
        Output {
            field: Some(Parity::Top),
            weave: None,
            before,
            after,
            at,
            of,
            matched: false,
        }
    }

    #[test]
    fn nothing_is_drawn_until_the_frame_after_it_arrives() {
        let mut schedule = Schedule::new(6);
        schedule.hold(0, vec![output(0, 1, Source::Previous, Source::Next)]);
        assert!(schedule.is_empty());
        schedule.hold(1, vec![output(0, 1, Source::Previous, Source::Next)]);
        assert_eq!(schedule.len(), 1);
        let ready = schedule.front().expect("展開された1フレーム");
        assert_eq!(ready.before, 0);
        assert_eq!(ready.cur, 0);
        assert_eq!(ready.after, 1);
    }

    #[test]
    fn the_last_frame_of_a_stream_is_not_lost() {
        let mut schedule = Schedule::new(6);
        schedule.hold(0, vec![output(0, 1, Source::Previous, Source::Next)]);
        schedule.expand(true);
        assert_eq!(schedule.len(), 1);
        let ready = schedule.front().expect("最後の1フレーム");
        assert_eq!(ready.after, 0, "後ろが無いので自分のフィールドで代用する");
    }

    #[test]
    fn a_slot_something_still_reads_is_not_free() {
        let mut schedule = Schedule::new(3);
        schedule.hold(0, vec![output(0, 1, Source::Previous, Source::Next)]);
        schedule.hold(1, vec![output(0, 1, Source::Previous, Source::Next)]);
        assert_eq!(schedule.free(), Some(2));
        schedule.pop();
        // 0 は直前のフレームなので前のフィールドとして読まれ得る。
        assert_eq!(schedule.free(), Some(2));
        schedule.clear();
        assert_eq!(schedule.free(), Some(0));
    }

    #[test]
    fn a_wider_reach_waits_one_more_frame_and_keeps_one_more() {
        let one = || vec![output(0, 1, Source::Previous, Source::Next)];
        let mut schedule = Schedule::with_reach(8, 2, 2);
        schedule.hold(0, one());
        schedule.hold(1, one());
        assert!(schedule.is_empty(), "後ろの二つ目がまだ");
        schedule.hold(2, one());
        let first = schedule.front().expect("1 フレーム目");
        assert_eq!(first.frames, [0, 0, 0, 1, 2], "前は無いので自分");
        schedule.pop();
        schedule.hold(3, one());
        schedule.hold(4, one());
        schedule.pop();
        let third = schedule.front().expect("3 フレーム目");
        assert_eq!(third.frames, [0, 1, 2, 3, 4]);
        assert_eq!((third.before, third.cur, third.after), (1, 2, 3));
        assert_eq!(schedule.free(), Some(5));
        schedule.expand(true);
        let last = (0..3).map(|_| {
            let ready = schedule.front().expect("残り");
            schedule.pop();
            ready.frames
        });
        assert_eq!(last.last(), Some([2, 3, 4, 4, 4]), "末尾は自分で埋める");
    }

    #[test]
    fn one_frame_can_become_three_outputs() {
        let mut schedule = Schedule::new(6);
        let three = vec![
            output(0, 3, Source::Previous, Source::Current),
            output(1, 3, Source::Current, Source::Next),
            output(2, 3, Source::Current, Source::Next),
        ];
        schedule.hold(0, three);
        schedule.hold(1, vec![output(0, 1, Source::Previous, Source::Next)]);
        assert_eq!(schedule.len(), 3);
    }
}
