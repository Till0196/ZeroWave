// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 再生中・録画中に、スクリーンセーバー・画面オフとスリープを止めるよう OS に頼む。
//! 止められるのは操作が無いまま時間がたったときだけで、ユーザーが自分で入れるスリープは止めない。
//! 頼む・取り下げるのは別のスレッドで（D-Bus の応答待ちでレンダリングを止めない）。

use std::sync::mpsc::{Sender, channel};

#[cfg(target_os = "linux")]
mod dbus;

#[cfg(any(target_os = "macos", target_os = "linux"))]
const APP: &str = "ZeroWave";
#[cfg(any(target_os = "windows", target_os = "linux"))]
const WHY: &str = "再生中または録画中";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keep {
    #[default]
    Nothing,
    /// スリープだけ止める（最小化した再生・録画）。
    System,
    /// スクリーンセーバー・画面オフとスリープを止める。
    Display,
}

#[derive(Debug, Default)]
pub struct Awake {
    now: Keep,
    to: Option<Sender<Keep>>,
}

impl Awake {
    pub fn set(&mut self, keep: Keep) {
        if keep == self.now {
            return;
        }
        self.now = keep;
        if self.to.is_none() {
            let (to, from) = channel::<Keep>();
            let spawned = std::thread::Builder::new()
                .name(String::from("awake"))
                .spawn(move || {
                    let mut held: Option<Held> = None;
                    while let Ok(keep) = from.recv() {
                        if held.take().is_some() {
                            tracing::info!("スリープを止めるのをやめた");
                        }
                        if keep != Keep::Nothing {
                            held = Held::take(keep);
                        }
                    }
                });
            if let Err(error) = spawned {
                tracing::warn!("スリープを止めるスレッドを作れない: {error}");
                return;
            }
            self.to = Some(to);
        }
        if let Some(to) = &self.to {
            let _ = to.send(keep);
        }
    }
}

struct Held {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    _held: os::Held,
}

impl Held {
    fn take(keep: Keep) -> Option<Self> {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            match os::Held::take(keep) {
                Ok(held) => {
                    tracing::info!("スリープを止めた（{keep:?}）");
                    Some(Self { _held: held })
                }
                Err(why) => {
                    tracing::warn!("スリープを止められない: {why}");
                    None
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = keep;
            None
        }
    }
}

/// 電源要求（`PowerCreateRequest`）。`powercfg /requests` に理由が出る。
#[cfg(target_os = "windows")]
mod os {
    use super::{Keep, WHY};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Power::{
        POWER_REQUEST_TYPE, PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired,
        PowerRequestSystemRequired, PowerSetRequest,
    };
    use windows::Win32::System::Threading::{
        POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
    };
    use windows::core::PWSTR;

    /// `POWER_REQUEST_CONTEXT_VERSION`（`minwinbase.h`）。
    const VERSION: u32 = 0;

    pub struct Held {
        request: HANDLE,
        kinds: Vec<POWER_REQUEST_TYPE>,
    }

    impl Held {
        pub fn take(keep: Keep) -> Result<Self, String> {
            let mut why = crate::window::win32::wide(WHY);
            let context = REASON_CONTEXT {
                Version: VERSION,
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                Reason: REASON_CONTEXT_0 {
                    SimpleReasonString: PWSTR(why.as_mut_ptr()),
                },
            };
            // SAFETY: `context` と `why` は呼び出しの間生きている。
            let request = unsafe { PowerCreateRequest(&context) }
                .map_err(|error| format!("PowerCreateRequest: {error}"))?;
            let mut held = Self {
                request,
                kinds: Vec::new(),
            };
            let kinds: &[POWER_REQUEST_TYPE] = match keep {
                Keep::Nothing => &[],
                Keep::System => &[PowerRequestSystemRequired],
                Keep::Display => &[PowerRequestSystemRequired, PowerRequestDisplayRequired],
            };
            for &kind in kinds {
                // SAFETY: `request` は上で作った電源要求。
                unsafe { PowerSetRequest(held.request, kind) }
                    .map_err(|error| format!("PowerSetRequest: {error}"))?;
                held.kinds.push(kind);
            }
            Ok(held)
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            for &kind in &self.kinds {
                // SAFETY: `request` に `kind` を立てたのはこの値。
                let _ = unsafe { PowerClearRequest(self.request, kind) };
            }
            // SAFETY: `PowerCreateRequest` のハンドルを一度だけ閉じる。
            let _ = unsafe { CloseHandle(self.request) };
        }
    }
}

/// IOKit の電源アサーション。画面のスリープを止めるとシステムのアイドルスリープも止まる。
#[cfg(target_os = "macos")]
mod os {
    use super::{APP, Keep};
    use objc2_core_foundation::CFString;
    use std::ffi::c_void;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: *const c_void,
            level: u32,
            name: *const c_void,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    const LEVEL_ON: u32 = 255;

    pub struct Held(u32);

    impl Held {
        pub fn take(keep: Keep) -> Result<Self, String> {
            let kind = CFString::from_str(match keep {
                Keep::Display => "PreventUserIdleDisplaySleep",
                Keep::System | Keep::Nothing => "PreventUserIdleSystemSleep",
            });
            // `pmset -g assertions` は ASCII でない名前を空で出す。
            let name = CFString::from_str(&format!("{APP}: playing or recording"));
            let mut id = 0;
            // SAFETY: どちらも生きている CFString で、`id` は書き込める。
            let status = unsafe {
                IOPMAssertionCreateWithName(
                    std::ptr::from_ref(&*kind).cast(),
                    LEVEL_ON,
                    std::ptr::from_ref(&*name).cast(),
                    &mut id,
                )
            };
            if status != 0 {
                return Err(format!("IOPMAssertionCreateWithName: {status:#x}"));
            }
            Ok(Self(id))
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            // SAFETY: 作ったアサーションを一度だけ手放す。
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }
}

/// スリープは logind の抑止（`Inhibit`）、スクリーンセーバーはデスクトップの抑止（GNOME は
/// `org.gnome.SessionManager`、ほかは `org.freedesktop.ScreenSaver`）。どれも D-Bus で、libdbus は実行時に開く。
#[cfg(target_os = "linux")]
mod os {
    use super::dbus::{Arg, Bus, Kind};
    use super::{APP, Keep, WHY};
    use std::os::fd::OwnedFd;

    /// `org.gnome.SessionManager.Inhibit` の flags（サスペンドとアイドル）。
    const GNOME_SUSPEND_IDLE: u32 = 4 | 8;

    pub struct Held {
        _sleep: Option<OwnedFd>,
        _screen: Option<Screen>,
    }

    impl Held {
        pub fn take(keep: Keep) -> Result<Self, String> {
            let sleep = inhibit_sleep(keep)
                .inspect_err(|why| tracing::warn!("logind に頼めない: {why}"))
                .ok();
            let screen = (keep == Keep::Display)
                .then(Screen::take)
                .and_then(|screen| {
                    screen
                        .inspect_err(|why| {
                            tracing::warn!("スクリーンセーバーを止められない: {why}")
                        })
                        .ok()
                });
            if sleep.is_none() && screen.is_none() {
                return Err(String::from("D-Bus で頼める相手が無い"));
            }
            Ok(Self {
                _sleep: sleep,
                _screen: screen,
            })
        }
    }

    fn inhibit_sleep(keep: Keep) -> Result<OwnedFd, String> {
        let what = match keep {
            Keep::Display => c"sleep:idle",
            Keep::System | Keep::Nothing => c"sleep",
        };
        let app = std::ffi::CString::new(APP).unwrap_or_default();
        let why = std::ffi::CString::new(WHY).unwrap_or_default();
        let bus = Bus::open(Kind::System)?;
        bus.call_for_fd(
            c"org.freedesktop.login1",
            c"/org/freedesktop/login1",
            c"org.freedesktop.login1.Manager",
            c"Inhibit",
            &[
                Arg::Text(what),
                Arg::Text(&app),
                Arg::Text(&why),
                Arg::Text(c"block"),
            ],
        )
    }

    /// 繋いだままにする（切れるとデスクトップが抑止を外す）。
    struct Screen {
        bus: Bus,
        cookie: u32,
        gnome: bool,
    }

    impl Screen {
        fn take() -> Result<Self, String> {
            let app = std::ffi::CString::new(APP).unwrap_or_default();
            let why = std::ffi::CString::new(WHY).unwrap_or_default();
            let bus = Bus::open(Kind::Session)?;
            let gnome = bus.call_for_u32(
                c"org.gnome.SessionManager",
                c"/org/gnome/SessionManager",
                c"org.gnome.SessionManager",
                c"Inhibit",
                &[
                    Arg::Text(&app),
                    Arg::U32(0),
                    Arg::Text(&why),
                    Arg::U32(GNOME_SUSPEND_IDLE),
                ],
            );
            let (cookie, gnome) = match gnome {
                Ok(cookie) => (cookie, true),
                Err(gnome) => {
                    let freedesktop = bus.call_for_u32(
                        c"org.freedesktop.ScreenSaver",
                        c"/org/freedesktop/ScreenSaver",
                        c"org.freedesktop.ScreenSaver",
                        c"Inhibit",
                        &[Arg::Text(&app), Arg::Text(&why)],
                    );
                    match freedesktop {
                        Ok(cookie) => (cookie, false),
                        Err(freedesktop) => return Err(format!("{gnome}／{freedesktop}")),
                    }
                }
            };
            Ok(Self { bus, cookie, gnome })
        }
    }

    impl Drop for Screen {
        fn drop(&mut self) {
            let _ = if self.gnome {
                self.bus.call(
                    c"org.gnome.SessionManager",
                    c"/org/gnome/SessionManager",
                    c"org.gnome.SessionManager",
                    c"Uninhibit",
                    &[Arg::U32(self.cookie)],
                )
            } else {
                self.bus.call(
                    c"org.freedesktop.ScreenSaver",
                    c"/org/freedesktop/ScreenSaver",
                    c"org.freedesktop.ScreenSaver",
                    c"UnInhibit",
                    &[Arg::U32(self.cookie)],
                )
            };
        }
    }
}
