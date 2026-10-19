// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映像を置く矩形と、ウィンドウの大きさ・縦横比の合わせ方。

use super::*;

impl Deck {
    pub(super) fn chrome(&self) -> f32 {
        self.bar
            + if self.file.ui.status_bar {
                self.status_height
            } else {
                0.0
            }
            + if self.scene.progress.is_some() {
                ui::theme::transport::HEIGHT
            } else {
                0.0
            }
            + self.panel_height()
    }

    /// 映像を出す場所を再生スレッドへ渡す。ポイントではなくピクセルで渡す（倍率がずれないように）。
    pub(super) fn tell_area(&mut self, ctx: &egui::Context, frame: egui::Rect) {
        let points = ctx.pixels_per_point().max(0.01);
        let left = (frame.left() * points).round().max(0.0) as u32;
        let top = (frame.top() * points).round().max(0.0) as u32;
        let right = (frame.right() * points).round().max(0.0) as u32;
        let bottom = (frame.bottom() * points).round().max(0.0) as u32;
        let width = right.saturating_sub(left).max(1);
        let height = bottom.saturating_sub(top).max(1);
        if self.told_area == Some((left, top, width, height)) {
            return;
        }
        self.told_area = Some((left, top, width, height));
        self.say(ToStage::Area {
            left,
            top,
            width,
            height,
        });
    }

    /// データ放送の文書の表示上の縦横比（横÷縦）。SD の文書はピクセルが正方形ではなく、
    /// `display-aspect-ratio` が無ければ映像の形に（ARIB STD-B24）。
    pub(super) fn page_ratio(&self, shown: &Shown) -> f32 {
        let (w, h) = shown.size;
        if let Some(aspect) = shown.aspect.filter(|aspect| *aspect > 0.0) {
            return aspect;
        }
        let sd = (w - 720.0).abs() < 1.0 && (h - 480.0).abs() < 1.0;
        match self.scene.ratio {
            Some(ratio) if sd && ratio > 0.0 => ratio,
            _ => w / h,
        }
    }

    /// 映像が実際に出ている矩形、ポイント単位（`Video::letterbox` と同じ計算）。
    pub(super) fn video_rect(&self, area: egui::Rect, ppp: f32) -> egui::Rect {
        let Some(ratio) = self.scene.ratio else {
            return area;
        };
        if ratio.is_nan() || ratio <= 0.0 || area.width() <= 0.0 || area.height() <= 0.0 {
            return area;
        }
        fit_rect(area, ratio, ppp)
    }
}

pub(super) fn strip_of<'a>(
    banded: &'a crate::draw::scene::Banded,
    theme: &ui::theme::Theme,
    rows: u32,
) -> ui::guide::mini::Strip<'a> {
    ui::guide::mini::Strip {
        channels: &banded.listing.channels,
        rows,
        now_when: banded.now_when.clone(),
        now_title: banded.now_title.clone(),
        band: banded.band,
        bands: &banded.bands,
        watching: banded.watching,
        from: banded.from,
        moment: Some(banded.moment),
        theme: *theme,
    }
}

/// `area` に縦横比 `wanted` の矩形を内接させる。端はピクセルで丸める（ポイントで丸めると映像と 1 ピクセルずれる）。
pub(super) fn fit_rect(area: egui::Rect, wanted: f32, ppp: f32) -> egui::Rect {
    let have = area.width() / area.height();
    let (width, height) = if have > wanted {
        (area.height() * wanted, area.height())
    } else {
        (area.width(), area.width() / wanted)
    };
    let left = area.left() + (area.width() - width) / 2.0;
    let top = area.top() + (area.height() - height) / 2.0;
    let ppp = if ppp > 0.0 { ppp } else { 1.0 };
    let snap = |v: f32| (v * ppp).round() / ppp;
    egui::Rect::from_min_max(
        egui::pos2(snap(left), snap(top)),
        egui::pos2(snap(left + width), snap(top + height)),
    )
}

impl Deck {
    /// ウィンドウの大きさが変わったのを見る。ドラッグ中は何もしない（押し戻すことになる）。
    /// 落ち着いたら、`keep_aspect` なら映像の縦横比に合わせる（Ctrl を押していたら逆）。
    pub(super) fn settle_ratio(&mut self, ctx: &egui::Context, window: &Arc<Window>) {
        let size = ctx.input(|input| input.viewport_rect().size());
        let now = (size.x.round() as u32, size.y.round() as u32);
        let was = self.size_was.replace(now);
        if was.is_some_and(|was| was != now) {
            // ドラッグ開始時の大きさを保つ（フレームごとの差だと値が往復する）。
            if self.sized_at.is_none() {
                let chrome = self.chrome().round() as u32;
                self.sizing_from =
                    was.map(|(width, height)| (width, height.saturating_sub(chrome)));
            }
            self.sized_at = Some(Instant::now());
            self.ctrl_sizing |= ctx.input(|input| input.modifiers.ctrl);
        }
        let Some(at) = self.sized_at else {
            return;
        };
        if at.elapsed() < SETTLE {
            ctx.request_repaint_after(SETTLE);
            return;
        }
        self.sized_at = None;
        let from = self.sizing_from.take();
        let ctrl = std::mem::take(&mut self.ctrl_sizing);
        if self.file.video.frame.keep_aspect == ctrl {
            return;
        }
        // 全画面と最大化は画面に合わせた大きさなので、縦横比では変えない。
        if window.fullscreen().is_some() || window.is_maximized() {
            return;
        }
        self.keep_ratio(window, from, now);
    }

    fn keep_ratio(&mut self, window: &Arc<Window>, from: Option<(u32, u32)>, now: (u32, u32)) {
        let chrome = self.chrome().round() as u32;
        let (width, height) = now;
        let area = height.saturating_sub(chrome);
        if area == 0 {
            return;
        }
        // パン&スキャンを掛けた後の形。
        let Some(ratio) = self
            .scene
            .ratio
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        else {
            return;
        };
        let (was_w, was_h) = from.unwrap_or((width, area));
        let (want_w, want_h) = if width.abs_diff(was_w) >= area.abs_diff(was_h) {
            (width, (width as f32 / ratio).round().max(1.0) as u32)
        } else {
            ((area as f32 * ratio).round().max(1.0) as u32, area)
        };
        if want_w.abs_diff(width) <= RATIO_SLACK && want_h.abs_diff(area) <= RATIO_SLACK {
            return;
        }
        let _ = window.request_inner_size(winit::dpi::LogicalSize::new(
            want_w as f64,
            f64::from(want_h + chrome),
        ));
    }

    /// ウィンドウを映像が等倍で出る大きさにする（1440×1080 の 16:9 は 1920）。`--fit-window` のときだけ。
    pub(super) fn fit_window(&mut self, window: &Arc<Window>) {
        if self.fitted {
            return;
        }
        let Some((width, height)) = self.scene.picture else {
            return;
        };
        self.fitted = true;
        let points = window.scale_factor();
        let chrome = (f64::from(self.chrome()) * points).round() as u32;
        let outer = window.outer_size();
        let inner = window.inner_size();
        let decorations = (
            outer.width.saturating_sub(inner.width),
            outer.height.saturating_sub(inner.height),
        );
        // 作業領域を取得できない画面では画面の 92% を目安にする。
        let room = work_area().unwrap_or_else(|| {
            let screen = window.current_monitor().map_or((1280, 720), |monitor| {
                let size = monitor.size();
                (size.width, size.height)
            });
            (
                (screen.0 as f32 * 0.92) as u32,
                (screen.1 as f32 * 0.92) as u32,
            )
        });
        let space = (
            room.0.saturating_sub(decorations.0).max(1),
            room.1
                .saturating_sub(decorations.1)
                .saturating_sub(chrome)
                .max(1),
        );
        let ratio = width as f32 / height.max(1) as f32;
        let (mut width, mut height) = (width, height);
        if width > space.0 {
            width = space.0;
            height = (space.0 as f32 / ratio).round().max(1.0) as u32;
        }
        if height > space.1 {
            height = space.1;
            width = (space.1 as f32 * ratio).round().max(1.0) as u32;
        }
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(
            width.max(1),
            height.max(1) + chrome,
        ));
    }
}

#[cfg(target_os = "windows")]
fn work_area() -> Option<(u32, u32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut rect = RECT::default();
    // SAFETY: `SPI_GETWORKAREA` が書くのは `RECT` 一つで、その大きさを渡している。
    let got = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(std::ptr::from_mut(&mut rect).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    got.ok()?;
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    (width > 0 && height > 0).then_some((width as u32, height as u32))
}

#[cfg(not(target_os = "windows"))]
fn work_area() -> Option<(u32, u32)> {
    None
}
