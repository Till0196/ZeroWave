// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 別ウィンドウを開く位置。値は物理ピクセルで、ウィンドウの外枠を含む。

use std::sync::atomic::{AtomicIsize, Ordering};

use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

/// メインウィンドウの HWND（Windows）。別のスレッドから出す確認ダイアログを中央に出すため。
static MAIN: AtomicIsize = AtomicIsize::new(0);

pub fn set_main(window: &Window) {
    #[cfg(windows)]
    if let Some(hwnd) = super::win32::raw_hwnd(window) {
        MAIN.store(hwnd, Ordering::Relaxed);
    }
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn main_hwnd() -> Option<isize> {
    Some(MAIN.load(Ordering::Relaxed)).filter(|hwnd| *hwnd != 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.w
    }

    fn bottom(&self) -> i32 {
        self.y + self.h
    }

    fn overlap(&self, other: &Rect) -> i64 {
        let w = (self.right().min(other.right()) - self.x.max(other.x)).max(0);
        let h = (self.bottom().min(other.bottom()) - self.y.max(other.y)).max(0);
        i64::from(w) * i64::from(h)
    }

    fn distance_to(&self, other: &Rect) -> i64 {
        let dx = i64::from((self.x + self.w / 2) - (other.x + other.w / 2));
        let dy = i64::from((self.y + self.h / 2) - (other.y + other.h / 2));
        dx * dx + dy * dy
    }
}

pub fn centred(owner: Rect, size: (i32, i32), work: Rect) -> (i32, i32) {
    let along = |start: i32, length: i32, inner: i32, lo: i32, hi: i32| {
        let at = start + (length - inner) / 2;
        if at < lo {
            lo
        } else if at + inner > hi {
            hi - inner
        } else {
            at
        }
    };
    (
        along(owner.x, owner.w, size.0, work.x, work.right()),
        along(owner.y, owner.h, size.1, work.y, work.bottom()),
    )
}

pub fn inside(placed: Rect, monitor: Rect) -> (i32, i32) {
    let along = |at: i32, length: i32, lo: i32, hi: i32| {
        if at < lo {
            lo
        } else if at + length > hi {
            hi - length
        } else {
            at
        }
    };
    (
        along(placed.x, placed.w, monitor.x, monitor.right()),
        along(placed.y, placed.h, monitor.y, monitor.bottom()),
    )
}

pub fn monitor_for(event_loop: &ActiveEventLoop, placed: Rect) -> Option<Rect> {
    let monitors: Vec<Rect> = event_loop
        .available_monitors()
        .map(|monitor| {
            let at = monitor.position();
            let size = monitor.size();
            Rect {
                x: at.x,
                y: at.y,
                w: size.width as i32,
                h: size.height as i32,
            }
        })
        .collect();
    let best = monitors
        .iter()
        .max_by_key(|monitor| monitor.overlap(&placed))
        .filter(|monitor| monitor.overlap(&placed) > 0);
    best.or_else(|| {
        monitors
            .iter()
            .min_by_key(|monitor| monitor.distance_to(&placed))
    })
    .copied()
}

pub fn outer(window: &Window) -> Option<Rect> {
    let at = window.outer_position().ok()?;
    let size = window.outer_size();
    Some(Rect {
        x: at.x,
        y: at.y,
        w: size.width as i32,
        h: size.height as i32,
    })
}

#[cfg(windows)]
pub fn work_area(window: &Window) -> Option<Rect> {
    super::win32::work_area(super::win32::hwnd(window)?)
}

#[cfg(not(windows))]
pub fn work_area(window: &Window) -> Option<Rect> {
    let monitor = window.current_monitor()?;
    let at = monitor.position();
    let size = monitor.size();
    Some(Rect {
        x: at.x,
        y: at.y,
        w: size.width as i32,
        h: size.height as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1040,
    };

    #[test]
    fn a_small_window_sits_in_the_middle_of_the_owner() {
        let owner = Rect {
            x: 100,
            y: 100,
            w: 1280,
            h: 720,
        };
        assert_eq!(centred(owner, (480, 320), SCREEN), (500, 300));
    }

    #[test]
    fn a_centred_window_stays_in_the_work_area() {
        let owner = Rect {
            x: 1500,
            y: 900,
            w: 640,
            h: 360,
        };
        assert_eq!(centred(owner, (800, 600), SCREEN), (1120, 440));
        let left = Rect {
            x: -400,
            y: -100,
            w: 640,
            h: 360,
        };
        assert_eq!(centred(left, (800, 600), SCREEN), (0, 0));
    }

    #[test]
    fn a_kept_place_is_pulled_back_onto_the_screen() {
        let placed = Rect {
            x: 1700,
            y: -20,
            w: 400,
            h: 300,
        };
        assert_eq!(inside(placed, SCREEN), (1520, 0));
        let fits = Rect {
            x: 200,
            y: 100,
            w: 400,
            h: 300,
        };
        assert_eq!(inside(fits, SCREEN), (200, 100));
    }
}
