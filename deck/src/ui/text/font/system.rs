// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! OS にフォントを問い合わせる。結果が無ければ空で、呼ぶ側がフォントの一覧から選ぶ。

/// OS が UI に使っているフォントの名前（日本語の文字を表示できるとは限らない）。
#[cfg(windows)]
pub fn ui_family() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        SystemParametersInfoW,
    };
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: 大きさを入れた構造体を渡し、OS がそこへ書く。
    unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some(std::ptr::from_mut(&mut metrics).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()?;
    let face = &metrics.lfMessageFont.lfFaceName;
    let length = face
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(face.len());
    let name = String::from_utf16_lossy(&face[..length]);
    (!name.is_empty()).then_some(name)
}

/// 日本語の文字の代わりに使うフォント（先に来る物ほど先）。Windows は `FontLink\SystemLink`。
#[cfg(windows)]
pub fn japanese_fallbacks(ui: Option<&str>) -> Vec<String> {
    use windows::Win32::System::Registry::RRF_RT_REG_MULTI_SZ;
    let Some(ui) = ui else {
        return Vec::new();
    };
    let Some(bytes) = crate::window::win32::registry_value(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\FontLink\SystemLink",
        ui,
        RRF_RT_REG_MULTI_SZ,
    ) else {
        return Vec::new();
    };
    let buffer: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    buffer
        .split(|unit| *unit == 0)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let line = String::from_utf16_lossy(line);
            line.split(',').nth(1).map(|name| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// macOS：CoreText に、UI のフォントで「あ」を表示するときの代替フォントを問い合わせる（`CTFontCreateForString`）。
#[cfg(target_os = "macos")]
pub fn ui_family() -> Option<String> {
    use objc2_core_foundation::{CFRange, CFRetained, CFString, CFType};
    use std::ffi::c_void;
    use std::ptr::NonNull;
    #[link(name = "CoreText", kind = "framework")]
    unsafe extern "C" {
        fn CTFontCreateUIFontForLanguage(
            kind: u32,
            size: f64,
            language: *const c_void,
        ) -> *mut c_void;
        fn CTFontCreateForString(
            font: *const c_void,
            string: *const c_void,
            range: CFRange,
        ) -> *mut c_void;
        fn CTFontCopyFamilyName(font: *const c_void) -> *mut c_void;
    }
    const SYSTEM: u32 = 2;
    let japanese = CFString::from_str("ja");
    let text = CFString::from_str("あ");
    // SAFETY: Create／Copy の戻り値は参照カウントが一つ増えた物で `CFRetained` が手放す。null は `None`。
    unsafe {
        let system =
            CTFontCreateUIFontForLanguage(SYSTEM, 0.0, std::ptr::from_ref(&*japanese).cast());
        let system: CFRetained<CFType> = CFRetained::from_raw(NonNull::new(system.cast())?);
        let fallback = CTFontCreateForString(
            std::ptr::from_ref(&*system).cast(),
            std::ptr::from_ref(&*text).cast(),
            CFRange {
                location: 0,
                length: 1,
            },
        );
        let fallback: CFRetained<CFType> = CFRetained::from_raw(NonNull::new(fallback.cast())?);
        let name = CTFontCopyFamilyName(std::ptr::from_ref(&*fallback).cast());
        let name: CFRetained<CFString> = CFRetained::from_raw(NonNull::new(name.cast())?);
        let name = name.to_string();
        (!name.is_empty() && !name.starts_with('.')).then_some(name)
    }
}

/// CoreText が返すのはフォントの一覧から探せない隠しフォントなので、同じフォントの探せる名前を並べる。
#[cfg(target_os = "macos")]
pub fn japanese_fallbacks(_ui: Option<&str>) -> Vec<String> {
    [
        "Hiragino Sans",
        "Hiragino Kaku Gothic ProN",
        "Hiragino Kaku Gothic Pro",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Linux と Android：fontconfig に日本語の sans-serif を問い合わせる（`fc-match`）。
#[cfg(not(any(windows, target_os = "macos")))]
pub fn ui_family() -> Option<String> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", "sans-serif:lang=ja"])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !name.is_empty()).then_some(name)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn japanese_fallbacks(_ui: Option<&str>) -> Vec<String> {
    Vec::new()
}
