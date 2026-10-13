// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フレームが実際に画面へ表示された時刻を取得する（DXGI の `GetFrameStatistics`）。Windows 以外では測らない。

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::collections::VecDeque;

    use windows::Win32::Graphics::Dxgi::{DXGI_FRAME_STATISTICS, IDXGISwapChain3};
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

    /// 保持する Present の数（ウィンドウが隠れている間は統計が止まるので少し余分に）。
    const KEPT: usize = 32;

    /// 何回分の実測から求めるか（二秒分）。
    const SMOOTH: usize = 120;

    const MOST_MS: f64 = 250.0;

    const NUDGE: f64 = 2.0;

    /// 平滑化する範囲の値がこれだけばらついている間は報告しない（10 パーセンタイルと 90 パーセンタイルの差）、ミリ秒。
    const SETTLED_MS: f64 = 6.0;

    #[derive(Debug)]
    pub struct Shown {
        sent: VecDeque<(u32, i64)>,
        seen: VecDeque<f64>,
        hz: f64,
        said: Option<f64>,
    }

    impl Default for Shown {
        fn default() -> Self {
            let mut hz = 0i64;
            let _ = unsafe { QueryPerformanceFrequency(&mut hz) };
            Self {
                sent: VecDeque::new(),
                seen: VecDeque::new(),
                hz: hz as f64,
                said: None,
            }
        }
    }

    fn now() -> i64 {
        let mut at = 0i64;
        let _ = unsafe { QueryPerformanceCounter(&mut at) };
        at
    }

    impl Shown {
        pub fn known(&self) -> bool {
            self.said.is_some()
        }

        /// 測ったものを捨てる（ウィンドウがドラッグされていた間の値を混ぜない）。報告している値は残す。
        pub fn forget(&mut self) {
            self.sent.clear();
            self.seen.clear();
        }

        pub fn presented(&mut self, surface: &wgpu::Surface<'static>) -> Option<f64> {
            if self.hz <= 0.0 {
                return None;
            }
            // SAFETY: 取得したサーフェスをこの場で読むだけ。D3D12 でなければ取得できない。
            let hal = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }?;
            let chain = hal.swap_chain()?;
            self.mark(&chain);
            self.ask(&chain)
        }

        fn mark(&mut self, chain: &IDXGISwapChain3) {
            let Ok(count) = (unsafe { chain.GetLastPresentCount() }) else {
                return;
            };
            self.sent.push_back((count, now()));
            while self.sent.len() > KEPT {
                self.sent.pop_front();
            }
        }

        fn ask(&mut self, chain: &IDXGISwapChain3) -> Option<f64> {
            let mut stats = DXGI_FRAME_STATISTICS::default();
            unsafe { chain.GetFrameStatistics(&raw mut stats) }.ok()?;
            let at = self
                .sent
                .iter()
                .find(|(count, _)| *count == stats.PresentCount)?;
            let ms = (stats.SyncQPCTime - at.1) as f64 * 1000.0 / self.hz;
            if !(0.0..=MOST_MS).contains(&ms) {
                self.sent.clear();
                return None;
            }
            while self
                .sent
                .front()
                .is_some_and(|(count, _)| *count <= stats.PresentCount)
            {
                self.sent.pop_front();
            }
            self.seen.push_back(ms);
            while self.seen.len() > SMOOTH {
                self.seen.pop_front();
            }
            if self.seen.len() < SMOOTH {
                return None;
            }
            // 中央値。平均だとウィンドウを動かした一回で報告値が跳ねる。
            let mut sorted: Vec<f64> = self.seen.iter().copied().collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).expect("測った値に NaN は無い"));
            let middle = sorted[sorted.len() / 2];
            let spread = sorted[sorted.len() * 9 / 10] - sorted[sorted.len() / 10];
            if spread > SETTLED_MS {
                return None;
            }
            if self.said.is_some_and(|said| (said - middle).abs() < NUDGE) {
                return None;
            }
            self.said = Some(middle);
            Some(middle)
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::Shown;

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Default)]
pub struct Shown {
    _nothing: (),
}

#[cfg(not(target_os = "windows"))]
impl Shown {
    pub fn known(&self) -> bool {
        false
    }
    pub fn forget(&mut self) {}
    pub fn presented(&mut self, _surface: &wgpu::Surface<'static>) -> Option<f64> {
        None
    }
}
