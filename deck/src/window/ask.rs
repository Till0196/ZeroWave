// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 「OK」か「キャンセル」かを確認するダイアログ（既定はキャンセル）。選ぶまで返らない。
//! 知らせるだけの警告（[`warn`]）は待たない。

/// 警告を出す。呼んだスレッドは待たない。
#[cfg(windows)]
pub fn warn(title: &str, text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MB_SETFOREGROUND};
    let (title, text) = (title.to_owned(), text.to_owned());
    let _ = std::thread::Builder::new()
        .name(String::from("warn"))
        .spawn(move || {
            let hook = centre_on_main();
            super::win32::message_box(&title, &text, MB_OK | MB_ICONWARNING | MB_SETFOREGROUND);
            drop(hook);
        });
}

/// macOS のダイアログはメインスレッドでしか出せないので、ここでは出さない。
#[cfg(target_os = "macos")]
pub fn warn(_title: &str, _text: &str) {}

/// Linux は `zenity`。無ければログに残るだけ。
#[cfg(not(any(windows, target_os = "macos")))]
pub fn warn(title: &str, text: &str) {
    let _ = std::process::Command::new("zenity")
        .arg("--warning")
        .arg(format!("--title={title}"))
        .arg(format!("--text={text}"))
        .spawn();
}

#[cfg(windows)]
pub fn ok_cancel(title: &str, text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST,
    };
    let hook = centre_on_main();
    let answer = super::win32::message_box(
        title,
        text,
        MB_OKCANCEL | MB_DEFBUTTON2 | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
    );
    drop(hook);
    answer == IDOK
}

/// このスレッドで次に開くメッセージボックスをメインウィンドウの中央へ動かす。オーナーは指定しない
/// （別のスレッドのウィンドウをオーナーにすると、二つのスレッドの入力がつながる）。
#[cfg(windows)]
fn centre_on_main() -> Option<Hook> {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HCBT_ACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
        SetWindowsHookExW, WH_CBT,
    };

    use super::win32::{window_rect, work_area};

    unsafe extern "system" fn moved(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HCBT_ACTIVATE as i32
            && let Some(main) = super::place::main_hwnd()
            && MOVED.with(|moved| !moved.replace(true))
        {
            let dialog = HWND(wparam.0 as *mut core::ffi::c_void);
            let main = HWND(main as *mut core::ffi::c_void);
            if let (Some(owner), Some(shown)) = (window_rect(main), window_rect(dialog)) {
                let work = work_area(main).unwrap_or(owner);
                let (x, y) = super::place::centred(owner, (shown.w, shown.h), work);
                // SAFETY: いま開こうとしているこのスレッドのウィンドウ。
                let _ = unsafe {
                    SetWindowPos(
                        dialog,
                        None,
                        x,
                        y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
            }
        }
        // SAFETY: 受けたものをそのまま次へ。
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    super::place::main_hwnd()?;
    MOVED.with(|moved| moved.set(false));
    // SAFETY: このスレッドだけのフック。外すのは [`Hook`] が落ちるとき。
    let hook =
        unsafe { SetWindowsHookExW(WH_CBT, Some(moved), None, GetCurrentThreadId()) }.ok()?;
    Some(Hook(hook))
}

#[cfg(windows)]
thread_local! {
    static MOVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(windows)]
struct Hook(windows::Win32::UI::WindowsAndMessaging::HHOOK);

#[cfg(windows)]
impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: 自分で掛けたフック。
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(self.0) };
    }
}

#[cfg(target_os = "macos")]
pub fn ok_cancel(title: &str, text: &str) -> bool {
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title(title)
        .set_description(text)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancel);
    matches!(
        pollster::block_on(dialog.show()),
        rfd::MessageDialogResult::Ok
    )
}

/// Linux は `zenity`。無ければ OK として進める。
#[cfg(not(any(windows, target_os = "macos")))]
pub fn ok_cancel(title: &str, text: &str) -> bool {
    let answer = std::process::Command::new("zenity")
        .arg("--question")
        .arg(format!("--title={title}"))
        .arg(format!("--text={text}"))
        .arg("--ok-label=OK")
        .arg("--cancel-label=キャンセル")
        .arg("--default-cancel")
        .status();
    match answer {
        Ok(status) => status.success(),
        Err(error) => {
            tracing::warn!("確認のダイアログが出せない（zenity が無い）: {error}");
            true
        }
    }
}
