// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 何か所もで使う Win32 の呼び出しの安全なラッパー。

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, REG_ROUTINE_FLAGS, REG_VALUE_TYPE, RegGetValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GWLP_HWNDPARENT, GWLP_WNDPROC, GetSystemMetrics, GetWindowRect,
    MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, MessageBoxW, SYSTEM_METRICS_INDEX, WNDPROC,
};
use windows::core::PCWSTR;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use super::place::Rect;

/// winit のウィンドウの HWND。
pub fn hwnd(window: &Window) -> Option<HWND> {
    raw_hwnd(window).map(|raw| HWND(raw as *mut core::ffi::c_void))
}

pub fn raw_hwnd(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// NUL 終端の UTF-16。
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rect_of(rect: RECT) -> Rect {
    Rect {
        x: rect.left,
        y: rect.top,
        w: rect.right - rect.left,
        h: rect.bottom - rect.top,
    }
}

/// ウィンドウの外枠の位置とサイズ（物理ピクセル）。
pub fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: 書き込み先はローカルの RECT。破棄されたウィンドウなら失敗するだけ。
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect_of(rect))
}

/// ウィンドウがいちばん多く重なるモニターの作業領域（タスクバーを除く）。
pub fn work_area(hwnd: HWND) -> Option<Rect> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: 書き込み先は cbSize を入れたローカルの MONITORINFO。破棄されたウィンドウでも近いモニターが返る。
    unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }
        .ok()
        .ok()?;
    Some(rect_of(info.rcWork))
}

pub fn system_metric(index: SYSTEM_METRICS_INDEX) -> i32 {
    // SAFETY: 引数だけを見て値を返す。
    unsafe { GetSystemMetrics(index) }
}

/// 閉じるまで返らない。オーナーウィンドウは無し。
pub fn message_box(title: &str, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let body = wide(text);
    let title = wide(title);
    // SAFETY: 二つの文字列は NUL 終端で、呼び出しの間生きている。
    unsafe { MessageBoxW(None, PCWSTR(body.as_ptr()), PCWSTR(title.as_ptr()), style) }
}

/// `HKEY_LOCAL_MACHINE` の値をバイトのまま。`kind` は `RRF_RT_REG_SZ` など。
pub fn registry_value(key: &str, value: &str, kind: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let key = wide(key);
    let value = wide(value);
    let mut size = 0u32;
    let mut found = REG_VALUE_TYPE::default();
    // SAFETY: サイズだけを問い合わせる（バッファは渡さない）。文字列は NUL 終端。
    let asked = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            kind,
            Some(&mut found),
            None,
            Some(&mut size),
        )
    };
    if asked.is_err() || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: `size` バイトのバッファと、そのサイズ。
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            kind,
            Some(&mut found),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if read.is_err() {
        return None;
    }
    buffer.truncate(size as usize);
    Some(buffer)
}

pub type Procedure = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// 差し替える前のウィンドウプロシージャ。[`subclass`] だけが作る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Original(isize);

/// このスレッドのウィンドウのプロシージャを `procedure` に差し替え、元を返す。戻すのは [`Original::restore`]。
pub fn subclass(hwnd: HWND, procedure: Procedure) -> Original {
    // SAFETY: ウィンドウは呼び出し元スレッドのもので、`procedure` はウィンドウプロシージャの型の関数。
    Original(unsafe { set_long(hwnd, GWLP_WNDPROC, procedure as usize as isize) })
}

/// オーナーを替える（オーナーのあるウィンドウは、出すとオーナーごと手前に来る）。`None` ならオーナー無し。
pub fn set_owner(window: &Window, owner: Option<&Window>) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let owner = owner.and_then(raw_hwnd).unwrap_or(0);
    // SAFETY: どちらもこのプロセスのウィンドウ。
    unsafe { set_long(hwnd, GWLP_HWNDPARENT, owner) };
}

/// 32 ビットでは `SetWindowLongPtrW` が無く、`SetWindowLongW` が同じ役をする。
/// # Safety
/// `SetWindowLongPtrW` と同じ。
unsafe fn set_long(
    hwnd: HWND,
    index: windows::Win32::UI::WindowsAndMessaging::WINDOW_LONG_PTR_INDEX,
    value: isize,
) -> isize {
    #[cfg(target_pointer_width = "64")]
    // SAFETY: 呼び出し側の保証のとおり。
    let old =
        unsafe { windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(hwnd, index, value) };
    #[cfg(target_pointer_width = "32")]
    // SAFETY: 呼び出し側の保証のとおり。32 ビットでは値が `i32` に収まる。
    let old = unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongW(hwnd, index, value as i32) as isize
    };
    old
}

impl Original {
    pub fn is_set(self) -> bool {
        self.0 != 0
    }

    pub fn call(self, hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: `set_long` が返した、このウィンドウの元のプロシージャ（0 なら None）。
        let original = unsafe { std::mem::transmute::<isize, WNDPROC>(self.0) };
        // SAFETY: 受けたメッセージをそのまま元のプロシージャへ。
        unsafe { CallWindowProcW(original, hwnd, message, wparam, lparam) }
    }

    pub fn restore(self, hwnd: HWND) {
        // SAFETY: 自分で差し替えたウィンドウへ、元のプロシージャを戻す。
        unsafe { set_long(hwnd, GWLP_WNDPROC, self.0) };
    }
}
