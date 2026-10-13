// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 同時に処理するジョブの管理（GPU を持たない）：入力のスロットにどの入力が入っているかと、出力のスロットごとの
//! ジョブ。入力は一度だけ書き込み、続くジョブ（今の入力が次のジョブの前）は同じスロットを指す。

use std::time::Instant;

#[derive(Debug, Clone)]
pub(super) struct Job {
    pub index: u64,
    pub value: u64,
    pub previous: usize,
    pub current: usize,
    pub since: Instant,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Plan {
    pub previous: usize,
    pub current: usize,
    pub middle: usize,
    pub pack: Vec<(usize, u64)>,
}

#[derive(Debug)]
pub(super) struct Desk {
    /// 入力のスロットごとに、入っている入力の番号。
    inputs: Vec<Option<u64>>,
    /// 出力のスロットごとのジョブ（完了しても、スロットを使い回すまで残す）。
    middles: Vec<Option<Job>>,
}

impl Desk {
    pub fn new(jobs: u32) -> Self {
        let inputs = zwsdk::video::process::inputs(jobs) as usize;
        Self {
            inputs: vec![None; inputs],
            middles: vec![None; jobs as usize],
        }
    }

    pub fn pending(&self) -> impl Iterator<Item = &Job> {
        self.middles.iter().flatten().filter(|job| !job.done)
    }

    pub fn find(&self, index: u64) -> Option<(usize, &Job)> {
        self.middles.iter().enumerate().find_map(|(at, job)| {
            job.as_ref()
                .filter(|job| job.index == index)
                .map(|j| (at, j))
        })
    }

    pub fn held(&self, middle: usize) -> Option<&Job> {
        self.middles.get(middle)?.as_ref()
    }

    /// 入力 `index`（前は `index − 1`）を要求するための割り当て。空きが無ければ無し。
    pub fn plan(&self, index: u64) -> Option<Plan> {
        let middle = self
            .middles
            .iter()
            .position(|job| job.as_ref().is_none_or(|job| job.done))?;
        // 完了していないジョブが読んでいるスロットには書き込み直さない（読むだけなら分けてよい）。
        let reading: Vec<usize> = self
            .pending()
            .flat_map(|job| [job.previous, job.current])
            .collect();
        let previous = index.saturating_sub(1);
        let mut chosen: Vec<(u64, usize)> = Vec::new();
        let mut pack = Vec::new();
        for want in [previous, index] {
            if chosen.iter().any(|(input, _)| *input == want) {
                continue;
            }
            if let Some(at) = self.inputs.iter().position(|input| *input == Some(want)) {
                chosen.push((want, at));
            }
        }
        for want in [previous, index] {
            if chosen.iter().any(|(input, _)| *input == want) {
                continue;
            }
            let at = (0..self.inputs.len())
                .filter(|at| !reading.contains(at) && !chosen.iter().any(|(_, used)| used == at))
                .min_by_key(|at| self.inputs[*at].map_or(0, |input| input + 1))?;
            chosen.push((want, at));
            pack.push((at, want));
        }
        let slot_of = |want: u64| chosen.iter().find(|(input, _)| *input == want).map(|c| c.1);
        Some(Plan {
            previous: slot_of(previous)?,
            current: slot_of(index)?,
            middle,
            pack,
        })
    }

    pub fn start(&mut self, plan: &Plan, index: u64, value: u64, now: Instant) {
        for (at, input) in &plan.pack {
            self.inputs[*at] = Some(*input);
        }
        self.middles[plan.middle] = Some(Job {
            index,
            value,
            previous: plan.previous,
            current: plan.current,
            since: now,
            done: false,
        });
    }

    /// 値 `value` のジョブが完了した（通知チャネルから）。要求していなければ false。
    pub fn finish(&mut self, value: u64) -> bool {
        match self
            .middles
            .iter_mut()
            .flatten()
            .find(|job| job.value == value && !job.done)
        {
            Some(job) => {
                job.done = true;
                true
            }
            None => false,
        }
    }

    pub fn refresh(&mut self, done: impl Fn(u64) -> bool) {
        for job in self.middles.iter_mut().flatten() {
            job.done |= done(job.value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(desk: &mut Desk, index: u64, value: u64) -> Plan {
        let plan = desk.plan(index).expect("空きがある");
        desk.start(&plan, index, value, Instant::now());
        plan
    }

    #[test]
    fn an_input_is_packed_once_for_two_jobs() {
        let mut desk = Desk::new(2);
        let first = start(&mut desk, 5, 1);
        assert_eq!(first.pack.len(), 2, "初めは前と今の 2 フレーム");
        let second = start(&mut desk, 6, 2);
        assert_eq!(second.pack, vec![(second.current, 6)], "5 はもう入っている");
        assert_eq!(second.previous, first.current, "同じスロットを前として読む");
        assert_ne!(second.middle, first.middle);
        assert_eq!(desk.pending().count(), 2);
    }

    #[test]
    fn a_full_desk_takes_no_more() {
        let mut desk = Desk::new(2);
        start(&mut desk, 1, 1);
        start(&mut desk, 2, 2);
        assert_eq!(desk.plan(3), None, "出力のスロットが二つとも使用中");
        assert!(desk.finish(1));
        assert!(!desk.finish(1), "二度は完了しない");
        assert!(!desk.finish(9), "要求していない値");
        let plan = desk.plan(3).expect("一つ空いた");
        assert_eq!(plan.middle, 0);
        assert_eq!(
            desk.held(0).map(|job| job.index),
            Some(1),
            "使い回す前に結果を取り出す"
        );
        assert_eq!(plan.pack.len(), 1);
        assert_eq!(desk.find(1).map(|(at, _)| at), Some(0));
    }

    #[test]
    fn inputs_being_read_are_not_overwritten() {
        let mut desk = Desk::new(2);
        start(&mut desk, 1, 1);
        assert_eq!(desk.plan(3), None);
        desk.refresh(|value| value <= 1);
        let plan = desk.plan(3).expect("完了したら空く");
        assert_eq!(
            plan.pack,
            vec![(2, 2), (0, 3)],
            "空のスロット、次に古い入力のスロット"
        );
    }

    #[test]
    fn the_oldest_free_input_is_overwritten() {
        let mut desk = Desk::new(2);
        start(&mut desk, 1, 1); // スロット 0 に 0、1 に 1
        desk.refresh(|_| true);
        start(&mut desk, 2, 2); // 1 はある。2 は空のスロット 2 へ
        desk.refresh(|_| true);
        let plan = desk.plan(3).expect("空いている");
        assert_eq!(plan.pack, vec![(0, 3)], "入力 0 のスロットを使い回す");
    }

    #[test]
    fn the_first_input_is_its_own_previous() {
        let mut desk = Desk::new(1);
        let plan = start(&mut desk, 0, 1);
        assert_eq!(plan.previous, plan.current);
        assert_eq!(plan.pack, vec![(0, 0)]);
    }

    #[test]
    fn one_job_at_a_time_still_works() {
        let mut desk = Desk::new(1);
        start(&mut desk, 1, 1);
        assert_eq!(desk.plan(2), None);
        desk.refresh(|value| value == 1);
        let plan = start(&mut desk, 2, 2);
        assert_eq!(plan.pack.len(), 1, "入力 1 は残っている");
        assert_eq!(desk.find(2).map(|(_, job)| job.value), Some(2));
        assert_eq!(
            desk.find(1).map(|(_, job)| job.value),
            None,
            "スロットを使い回した"
        );
    }
}
