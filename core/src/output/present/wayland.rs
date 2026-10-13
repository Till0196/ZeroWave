// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Wayland のウィンドウへ EGL でレンダリングするための `wl_egl_window`。`wl_surface` は大きさを持たないので、
//! レンダリングする大きさはこれが持ち、ウィンドウの大きさが変わったら伝える。`libwayland-egl.so.1` は実行時に開く。

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::dl::Object;
use crate::output::video::Error;

/// ウィンドウの大きさの受け渡し。上位 32 ビットが幅、下位が高さ。ウィンドウのスレッドが書き、レンダリングのスレッドが読む。
#[derive(Debug, Clone, Default)]
pub struct Size(Arc<AtomicU64>);

impl Size {
    pub fn new(width: u32, height: u32) -> Self {
        let size = Self::default();
        size.set(width, height);
        size
    }

    pub fn set(&self, width: u32, height: u32) {
        let packed = (u64::from(width.max(1)) << 32) | u64::from(height.max(1));
        self.0.store(packed, Ordering::Relaxed);
    }

    pub fn get(&self) -> (u32, u32) {
        let packed = self.0.load(Ordering::Relaxed);
        ((packed >> 32) as u32, packed as u32)
    }
}

#[derive(Debug)]
pub struct EglWindow {
    _library: Object,
    resize_window: unsafe extern "C" fn(*mut c_void, i32, i32, i32, i32),
    destroy_window: unsafe extern "C" fn(*mut c_void),
    window: *mut c_void,
    size: Size,
    applied: (u32, u32),
}

impl EglWindow {
    /// # Safety
    /// `surface` は開いている `wl_surface *` で、返った `wl_egl_window` より長く生きること。
    pub unsafe fn new(surface: *mut c_void, size: Size) -> Result<Self, Error> {
        let library = Object::open_any(&["libwayland-egl.so.1", "libwayland-egl.so"])
            .map_err(|missing| Error::NoDevice(missing.0))?;
        let missing = |missing: crate::dl::Missing| Error::NoDevice(missing.0);
        // SAFETY: 名前は `wayland-egl-core.h` のエントリーポイントで、型はそのシグネチャ。
        let (create, resize_window, destroy_window) = unsafe {
            (
                library
                    .symbol::<unsafe extern "C" fn(*mut c_void, i32, i32) -> *mut c_void>(
                        b"wl_egl_window_create\0",
                    )
                    .map_err(missing)?,
                library
                    .symbol::<unsafe extern "C" fn(*mut c_void, i32, i32, i32, i32)>(
                        b"wl_egl_window_resize\0",
                    )
                    .map_err(missing)?,
                library
                    .symbol::<unsafe extern "C" fn(*mut c_void)>(b"wl_egl_window_destroy\0")
                    .map_err(missing)?,
            )
        };
        let applied = size.get();
        // SAFETY: 呼び出し側が `surface` を保証している。
        let window = unsafe { create(surface, applied.0 as i32, applied.1 as i32) };
        if window.is_null() {
            return Err(Error::NoDevice(String::from("wl_egl_window を作れません")));
        }
        Ok(Self {
            _library: library,
            resize_window,
            destroy_window,
            window,
            size,
            applied,
        })
    }

    pub fn native(&self) -> *mut c_void {
        self.window
    }

    /// ウィンドウの大きさが変わっていたら伝える。次の `eglSwapBuffers` から効く。
    pub fn follow(&mut self) -> (u32, u32) {
        let wanted = self.size.get();
        if wanted != self.applied {
            // SAFETY: 作った `wl_egl_window` で、まだ破棄していない。
            unsafe { (self.resize_window)(self.window, wanted.0 as i32, wanted.1 as i32, 0, 0) };
            self.applied = wanted;
        }
        wanted
    }
}

impl Drop for EglWindow {
    fn drop(&mut self) {
        // SAFETY: 作った `wl_egl_window` で、破棄するのは一度きり。EGL のサーフェスは先に破棄してある（`Present::drop`）。
        unsafe { (self.destroy_window)(self.window) };
    }
}
