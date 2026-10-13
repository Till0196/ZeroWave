// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GLES 2.0 を直接使う出力（`--render gles`）。ウィンドウの上に EGL のレンダラーを作る。

use std::sync::Arc;

use winit::window::Window;
use zwcore::screen::Painter;

pub fn open_gles(window: &Arc<Window>, vsync: bool) -> Option<Painter> {
    use winit::raw_window_handle::{
        HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    };
    use zwcore::output::present::{Native, WindowSize};

    let handle = match window.window_handle() {
        Ok(handle) => handle.as_raw(),
        Err(error) => {
            tracing::warn!("ウィンドウのハンドルを取得できない: {error}");
            return None;
        }
    };
    // Wayland はウィンドウ側の接続で EGL を開く（`wl_surface` はその接続の物）。
    let display = window.display_handle().ok().map(|display| display.as_raw());
    let size = window.inner_size();
    let size = (size.width.max(1), size.height.max(1));
    let native = match handle {
        RawWindowHandle::Xlib(handle) => Native::X11 {
            window: handle.window as *mut std::ffi::c_void,
        },
        RawWindowHandle::Xcb(handle) => Native::X11 {
            window: u64::from(handle.window.get()) as *mut std::ffi::c_void,
        },
        RawWindowHandle::Wayland(handle) => {
            let Some(RawDisplayHandle::Wayland(display)) = display else {
                tracing::warn!("Wayland のウィンドウなのに wl_display を取得できない");
                return None;
            };
            Native::Wayland {
                surface: handle.surface.as_ptr(),
                display: display.display.as_ptr(),
                size: WindowSize::new(size.0, size.1),
            }
        }
        RawWindowHandle::AndroidNdk(handle) => Native::Android {
            window: handle.a_native_window.as_ptr(),
        },
        other => {
            tracing::info!("このウィンドウの種類には EGL が未対応: {other:?}");
            return None;
        }
    };
    Some(Painter::Gles {
        native,
        size,
        vsync,
        window: Default::default(),
    })
}

/// EGL のコンテキストとウィンドウのサーフェスを一度作って捨てる（auto で GLES を選んだとき、使えるかを
/// レンダリングのスレッドに渡す前に確かめる）。
pub fn probe(window: &Arc<Window>) -> Result<(), String> {
    let Some(Painter::Gles { native, .. }) = open_gles(window, false) else {
        return Err(String::from("このウィンドウには EGL を使えない"));
    };
    // SAFETY: winit がこのスレッドで開いたウィンドウ。作ったコンテキストとサーフェスはここで捨てる。
    unsafe { zwcore::output::present::Window::on_window(native) }
        .map(drop)
        .map_err(|error| format!("{error}"))
}
