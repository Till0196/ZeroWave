// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GPU のデバイスを失ったとき（ドライバーの更新・リセット・取り外し）にレンダリングのデバイスを開き直す。

use super::*;

const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MOST: Duration = Duration::from_secs(30);
/// 続けてこの回数開けなければ通知する（その後も間を空けて試し続ける）。
const TELL_AFTER: u32 = 3;
/// サーフェスがこの間ずっと取れなければ、失ったものとして開き直す。
const SURFACE_PATIENCE: Duration = Duration::from_secs(3);
/// 開いてからこの間に失ったら「すぐ失った」と数える。
const QUICK_LOSS: Duration = Duration::from_secs(10);
/// すぐ失うことがこの回数続いたら開き直しをやめる（同じ GPU とドライバーでは開き直しても同じく失う）。
const GIVE_UP_AFTER: u32 = 3;

#[derive(Debug, Default)]
pub(super) struct Recovery {
    /// 失って開き直している。開けるまでは開けなくても終わらない。
    pub(super) active: bool,
    failures: u32,
    next: Option<Instant>,
    told: bool,
    failing_since: Option<Instant>,
    surface_lost: bool,
    test: Option<TestLoss>,
    test_fail: u32,
    /// 最後に開いた時刻。
    opened_at: Option<Instant>,
    /// 開いてすぐ失ったのが続いた回数。
    quick_losses: u32,
    /// 開き直しをやめた。ウィンドウは出したまま、GPU は使わない。
    pub(super) gave_up: bool,
}

/// 試すためだけ：`ZW_TEST_DEVICE_LOST=秒[x回数]` で開いてから秒数たつと失わせ、
/// `ZW_TEST_GPU_REOPEN_FAIL=回数` で開き直しをその回数だけ失敗させる。
#[derive(Debug)]
struct TestLoss {
    after: Duration,
    left: u32,
    at: Option<Instant>,
}

impl Recovery {
    pub(super) fn from_env() -> Self {
        let test = std::env::var("ZW_TEST_DEVICE_LOST").ok().and_then(|said| {
            let (seconds, times) = said.split_once('x').unwrap_or((&said, "1"));
            Some(TestLoss {
                after: Duration::from_secs_f64(seconds.trim().parse().ok()?),
                left: times.trim().parse().ok()?,
                at: None,
            })
        });
        let test_fail = std::env::var("ZW_TEST_GPU_REOPEN_FAIL")
            .ok()
            .and_then(|said| said.trim().parse().ok())
            .unwrap_or(0);
        Self {
            test,
            test_fail,
            ..Self::default()
        }
    }

    pub(super) fn opened(&mut self) {
        if let Some(test) = self.test.as_mut().filter(|test| test.left > 0) {
            test.at = Some(Instant::now() + test.after);
        }
        self.failing_since = None;
        self.surface_lost = false;
        self.opened_at = Some(Instant::now());
    }

    /// 失った。開いてすぐ失うのが続いて、開き直しをやめるべきなら真。
    fn lost_again(&mut self) -> bool {
        let quick = self.opened_at.is_some_and(|at| at.elapsed() < QUICK_LOSS);
        self.quick_losses = if quick { self.quick_losses + 1 } else { 0 };
        self.opened_at = None;
        self.quick_losses >= GIVE_UP_AFTER
    }

    /// サーフェスを取れなかった。続けて取れない最初の一回なら真（記録はそのときだけ）。
    pub(super) fn surface_failed(&mut self, lost: bool) -> bool {
        self.surface_lost |= lost;
        let first = self.failing_since.is_none();
        self.failing_since.get_or_insert_with(Instant::now);
        first
    }

    pub(super) fn surface_ok(&mut self) {
        self.failing_since = None;
    }

    fn surface_gone(&self) -> bool {
        self.surface_lost
            || self
                .failing_since
                .is_some_and(|since| since.elapsed() >= SURFACE_PATIENCE)
    }

    fn test_due(&mut self) -> bool {
        let Some(test) = self.test.as_mut() else {
            return false;
        };
        if test.at.is_none_or(|at| Instant::now() < at) {
            return false;
        }
        test.at = None;
        test.left = test.left.saturating_sub(1);
        true
    }

    pub(super) fn injected_failure(&mut self) -> Option<String> {
        if !self.active || self.test_fail == 0 {
            return None;
        }
        self.test_fail -= 1;
        Some(String::from("試験のため失敗させた"))
    }
}

impl Stage {
    /// レンダリングの前に、GPU を失っていないか見る。失っていれば開き直しを要求して真。
    pub(super) fn watch_lost(&mut self) -> bool {
        #[cfg(unix)]
        if matches!(self.painter, Some(Painter::Gles { .. })) {
            if zwcore::output::present::context_lost() {
                // GLES はウィンドウごとに EGL のコンテキストを持つので、起動し直して全部作り直す。
                tracing::warn!("EGL のコンテキストを失った。起動し直す");
                crate::config::request_restart();
                self.save_volume();
                self.quit();
                return true;
            }
            return false;
        }
        let Some(Painter::Wgpu { gpu, .. }) = self.painter.as_ref() else {
            return false;
        };
        if self.recovery.test_due() {
            zwcore::output::video::gpu::lose_for_test(&gpu.device);
        }
        let why = zwcore::output::video::gpu::lost().or_else(|| {
            self.recovery
                .surface_gone()
                .then(|| String::from("サーフェスを取得できない"))
        });
        let Some(why) = why else {
            return false;
        };
        if self.recovery.lost_again() {
            self.give_up_gpu(&why);
            return true;
        }
        tracing::warn!("GPU デバイスを失った（{why}）。開き直す");
        self.recovery.active = true;
        self.recovery.next = None;
        self.rebuild_gpu(self.choices.backend);
        true
    }

    /// 開いてすぐ失うのが続いた。開き直しをやめ、GPU を使わずにウィンドウは出したままにする（ウィンドウを閉じれば終わる）。
    fn give_up_gpu(&mut self, why: &str) {
        tracing::error!(
            "GPU デバイスを開いてすぐ失うのが {GIVE_UP_AFTER} 回続いたので開き直しをやめる（{why}）"
        );
        self.recovery.gave_up = true;
        self.recovery.active = false;
        self.recovery.next = None;
        self.drop_gpu();
        crate::window::ask::warn(
            "ZeroWave",
            &format!(
                "GPU を使えなくなったため、映像とメニューを表示できません。\n\
                 GPU のドライバーが正しくインストールされているか確認してください。\n\n\
                 詳細: {why}"
            ),
        );
    }

    /// 開き直しに失敗したあと、間を空けてもう一度要求する。
    pub(super) fn retry_gpu(&mut self) {
        if !self.recovery.active || self.recovery.next.is_none_or(|next| Instant::now() < next) {
            return;
        }
        self.recovery.next = None;
        tracing::info!(
            "GPU デバイスを開き直す（{} 回目）",
            self.recovery.failures + 1
        );
        self.rebuild_gpu(self.choices.backend);
    }

    pub(super) fn reopen_failed(&mut self, error: &str) {
        let recovery = &mut self.recovery;
        recovery.failures += 1;
        let wait = RETRY_FIRST
            .saturating_mul(1 << (recovery.failures - 1).min(5))
            .min(RETRY_MOST);
        recovery.next = Some(Instant::now() + wait);
        tracing::warn!(
            "GPU デバイスを開き直せない（{} 回目、{} 秒後に再試行）: {error}",
            recovery.failures,
            wait.as_secs()
        );
        if recovery.failures >= TELL_AFTER && !recovery.told {
            recovery.told = true;
            tracing::error!("GPU デバイスを {TELL_AFTER} 回続けて開き直せない: {error}");
            self.toast(&said_with(
                "GPU を開き直せません。\n再試行を続けています。",
                &error,
            ));
            // トーストは GPU で描くので、開けない間は見えない。
            crate::window::ask::warn(
                "ZeroWave",
                &format!(
                    "GPU を開き直せないため、映像とメニューを表示できません。\n\
                     開き直しを続けています。\n\n\
                     詳細: {error}"
                ),
            );
        }
    }

    /// 開き直せた。真なら失ったあとの開き直し。
    pub(super) fn reopened(&mut self) -> bool {
        let recovery = &mut self.recovery;
        if !recovery.active {
            return false;
        }
        tracing::info!(
            "GPU デバイスを開き直した（失敗 {} 回のあと）",
            recovery.failures
        );
        recovery.active = false;
        recovery.failures = 0;
        recovery.next = None;
        recovery.told = false;
        true
    }
}
