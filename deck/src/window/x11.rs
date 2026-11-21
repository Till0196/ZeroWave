// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! X11 のウィンドウとポインターの情報を取得する。X11 でなければどれも `None`。

use winit::window::Window;

/// winit のウィンドウの X11 のウィンドウ ID。
#[cfg(target_os = "linux")]
pub fn window_id(window: &Window) -> Option<u32> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        _ => None,
    }
}

/// 右クリックメニューを画面全体を覆うウィンドウに表示できるなら、ポインターの位置（画面の座標）を返す。
/// 表示できるのは、背景の透過に要るコンポジターが動いているとき（`_NET_WM_CM_S<画面番号>` の所有者がいる）。
/// 位置は X サーバーから取得する。egui が保持している位置には、前のメニューの表示中に動かした分が反映されていない。
#[cfg(target_os = "linux")]
pub fn cover_at(window: &Window) -> Option<(i32, i32)> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    window_id(window)?;
    let (connection, screen) = x11rb::connect(None).ok()?;
    let name = format!("_NET_WM_CM_S{screen}");
    let atom = connection
        .intern_atom(false, name.as_bytes())
        .ok()?
        .reply()
        .ok()?
        .atom;
    if connection
        .get_selection_owner(atom)
        .ok()?
        .reply()
        .ok()?
        .owner
        == x11rb::NONE
    {
        return None;
    }
    let root = connection.setup().roots.get(screen)?.root;
    let pointer = connection.query_pointer(root).ok()?.reply().ok()?;
    Some((i32::from(pointer.root_x), i32::from(pointer.root_y)))
}

#[cfg(not(target_os = "linux"))]
pub fn cover_at(_window: &Window) -> Option<(i32, i32)> {
    None
}

/// ウィンドウのビジュアルの深さ。アルファで背景を透過できるのは 32 のとき。
#[cfg(target_os = "linux")]
pub fn depth(window: &Window) -> Option<u8> {
    use x11rb::protocol::xproto::ConnectionExt as _;
    let id = window_id(window)?;
    let (connection, _) = x11rb::connect(None).ok()?;
    Some(connection.get_geometry(id).ok()?.reply().ok()?.depth)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub fn depth(_window: &Window) -> Option<u8> {
    None
}
