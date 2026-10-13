// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ウィンドウ内のカーソル位置を OS に直接問い合わせる（ファイルをドラッグしている間は winit が通知しない）。メインスレッドで呼ぶ。

use winit::window::Window;

#[cfg(target_os = "windows")]
pub fn inside(window: &Window) -> Option<(f32, f32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let hwnd = super::win32::hwnd(window)?;
    let mut point = POINT::default();
    // SAFETY: 有効なウィンドウの HWND と、書き先の POINT。
    unsafe {
        GetCursorPos(&mut point).ok()?;
        if !ScreenToClient(hwnd, &mut point).as_bool() {
            return None;
        }
    }
    let scale = window.scale_factor() as f32;
    Some((point.x as f32 / scale, point.y as f32 / scale))
}

#[cfg(target_os = "macos")]
pub fn inside(window: &Window) -> Option<(f32, f32)> {
    use objc2_app_kit::NSView;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: winit のウィンドウの view。メインスレッドで、ウィンドウが有効な間だけ借りる。
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let ns_window = view.window()?;
    let at_window = ns_window.mouseLocationOutsideOfEventStream();
    let at_view = view.convertPoint_fromView(at_window, None);
    let height = view.bounds().size.height;
    let y = if view.isFlipped() {
        at_view.y
    } else {
        height - at_view.y
    };
    Some((at_view.x as f32, y as f32))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn inside(_window: &Window) -> Option<(f32, f32)> {
    None
}
