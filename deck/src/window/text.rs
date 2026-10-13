// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 番組説明のテキストを OS のテキストコントロール（RichEdit、`NSTextView`）で表示する。コントロールはウィンドウを持つスレッドでしか触れない
//! ので、別ウィンドウのスレッドは要求するだけ（`Request::Text`）。Linux は egui のテキストのまま。

use std::sync::Arc;

use winit::window::Window;

#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub rect: (i32, i32, i32, i32),
    pub sections: Vec<(String, String)>,
    /// 本文の囲みごとのバッジの文字色と背景色（コントロールは egui の上に重なるので、バッジもコントロールの中に出す）。
    pub marks: Vec<(String, [u8; 3])>,
    pub scroll: bool,
    pub dark: bool,
    pub colour: [u8; 3],
    pub dim: [u8; 3],
    pub back: [u8; 3],
    pub font_pt: f32,
    pub mono: bool,
    pub face: Option<crate::ui::text::font::NativeFace>,
    pub tail: bool,
}

#[derive(Default)]
pub struct Text {
    boxes: Vec<Piece>,
}

struct Piece {
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    inner: platform::Inner,
    last: Option<Shown>,
}

impl std::fmt::Debug for Text {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Text")
            .field("boxes", &self.boxes.len())
            .finish()
    }
}

impl Text {
    /// 変わったところだけ触る（文字列を入れ直すと選択が外れ、スクロールが先頭に戻る）。
    pub fn show(&mut self, window: &Arc<Window>, shown: &[Shown]) {
        for (n, shown) in shown.iter().enumerate() {
            if self.boxes.len() <= n {
                let Some(inner) = platform::Inner::open(window, shown.scroll) else {
                    return;
                };
                self.boxes.push(Piece { inner, last: None });
            }
            let piece = &mut self.boxes[n];
            if piece
                .last
                .as_ref()
                .is_some_and(|last| last.scroll != shown.scroll)
            {
                let Some(inner) = platform::Inner::open(window, shown.scroll) else {
                    return;
                };
                *piece = Piece { inner, last: None };
            }
            piece.show(window, shown);
        }
        for piece in self.boxes.iter_mut().skip(shown.len()) {
            if piece.last.take().is_some() {
                piece.inner.hide();
            }
        }
    }
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
struct Flat {
    text: String,
    headings: Vec<(usize, usize)>,
    marks: Vec<(usize, usize)>,
    units: usize,
}

impl Flat {
    fn push(&mut self, piece: &str) {
        self.text.push_str(piece);
        self.units += piece.encode_utf16().count();
    }
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn flatten(shown: &Shown, newline: &str) -> Flat {
    let mut flat = Flat {
        text: String::new(),
        headings: Vec::new(),
        marks: Vec::new(),
        units: 0,
    };
    for (n, (heading, body)) in shown.sections.iter().enumerate() {
        if n > 0 {
            flat.push(newline);
            flat.push(newline);
        }
        if !heading.is_empty() {
            let from = flat.units;
            flat.push(heading);
            flat.headings.push((from, flat.units - from));
            flat.push(newline);
        }
        let (body, found) = unmarked(&body.replace('\n', newline));
        let base = flat.units;
        flat.marks
            .extend(found.into_iter().map(|(from, len)| (base + from, len)));
        flat.push(&body);
    }
    flat
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
struct Badge {
    small: f64,
    height: f64,
    pad: f64,
    radius: f64,
    middle: f64,
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn badge(font: f64) -> Badge {
    let small = font * 0.8;
    Badge {
        small,
        height: small * 1.2,
        pad: (small * 0.15).max(1.0),
        radius: (small * 0.1).max(1.0),
        middle: font * 0.4,
    }
}

fn unmarked(text: &str) -> (String, Vec<(usize, usize)>) {
    use crate::ui::guide::detail::{MARK_CLOSE, MARK_OPEN};
    let mut out = String::with_capacity(text.len());
    let mut marks = Vec::new();
    let mut at = 0usize;
    let mut open: Option<usize> = None;
    for c in text.chars() {
        match c {
            MARK_OPEN => open = Some(at),
            MARK_CLOSE => {
                if let Some(from) = open.take() {
                    marks.push((from, at - from));
                }
            }
            _ => {
                out.push(c);
                at += c.len_utf16();
            }
        }
    }
    (out, marks)
}

fn appended<'a>(before: &Shown, now: &'a Shown) -> Option<&'a str> {
    let ([(had_heading, had)], [(heading, body)]) =
        (before.sections.as_slice(), now.sections.as_slice())
    else {
        return None;
    };
    if had_heading != heading || !now.marks.is_empty() {
        return None;
    }
    body.strip_prefix(had.as_str())
}

impl Piece {
    fn show(&mut self, window: &Arc<Window>, shown: &Shown) {
        let before = self.last.as_ref();
        let restyled = before.is_none_or(|before| {
            before.dark != shown.dark
                || before.colour != shown.colour
                || before.dim != shown.dim
                || before.back != shown.back
                || before.font_pt != shown.font_pt
                || before.mono != shown.mono
                || before.face != shown.face
                || before.marks != shown.marks
        });
        if restyled {
            self.inner.style(window, shown);
        }
        let appended = match before {
            Some(before) if !restyled && shown.tail => appended(before, shown),
            _ => None,
        };
        if let Some(more) = appended {
            if !more.is_empty() {
                self.inner.append(more);
            }
        } else if restyled || before.is_none_or(|before| before.sections != shown.sections) {
            self.inner.set_text(shown);
        }
        if before.is_none_or(|before| before.rect != shown.rect) {
            self.inner.place(shown.rect);
        }
        if before.is_none() {
            self.inner.reveal();
        }
        self.last = Some(shown.clone());
    }
}

#[cfg(windows)]
mod platform {
    //! Windows: 読み取り専用の RichEdit。Esc は握りつぶす（RichEdit は Esc で親に `WM_CLOSE` を投げる）。

    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::RichEdit::{
        CFE_BOLD, CFM_BOLD, CFM_CHARSET, CFM_COLOR, CFM_FACE, CFM_SIZE, CHARFORMATW, CHARRANGE,
        EM_EXGETSEL, EM_EXSETSEL, EM_GETLANGOPTIONS, EM_SETBKGNDCOLOR, EM_SETCHARFORMAT,
        EM_SETEVENTMASK, EM_SETLANGOPTIONS, EN_MSGFILTER, EN_SELCHANGE, ENM_KEYEVENTS,
        ENM_MOUSEEVENTS, ENM_SELCHANGE, ES_NOOLEDRAGDROP, IMF_AUTOFONT, IMF_DUALFONT, MSGFILTER,
        SCF_ALL, SCF_SELECTION, SEL_OBJECT, SELCHANGE,
    };

    use windows::Win32::UI::Controls::{NMHDR, SetWindowTheme};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState};
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DestroyMenu, DestroyWindow, ES_AUTOVSCROLL,
        ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, GetCursorPos, HMENU, MF_STRING, SW_HIDE,
        SW_SHOWNA, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos, SetWindowTextW,
        ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE, WINDOW_STYLE,
        WM_CLOSE, WM_COPY, WM_KEYDOWN, WM_KEYUP, WM_NOTIFY, WM_RBUTTONUP, WM_SETREDRAW, WS_CHILD,
        WS_CLIPSIBLINGS, WS_VSCROLL,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        HideCaret, WM_CHAR, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_SETFOCUS,
    };
    use windows::core::{PCWSTR, w};
    use winit::window::Window;

    use crate::window::win32::{self, Original, wide};

    const VK_ESCAPE: usize = 0x1b;
    const EM_SETSEL: u32 = 0x00b1;
    const EM_SETREADONLY: u32 = 0x00cf;
    const EM_REPLACESEL: u32 = 0x00c2;
    const EM_GETFIRSTVISIBLELINE: u32 = 0x00ce;
    const EM_LINESCROLL: u32 = 0x00b6;
    const WM_VSCROLL: u32 = 0x0115;
    const SB_BOTTOM: usize = 7;
    const COMMAND_COPY: usize = 1;
    const COMMAND_SELECT_ALL: usize = 2;

    struct Parent {
        original: Original,
        edits: Vec<isize>,
    }

    thread_local! {
        static PARENTS: RefCell<HashMap<isize, Parent>> = RefCell::new(HashMap::new());
        static EDITS: RefCell<HashMap<isize, Original>> = RefCell::new(HashMap::new());
    }

    /// コントロールの選択範囲を設定する（`EM_EXSETSEL`）。
    fn select(edit: HWND, range: CHARRANGE) {
        // SAFETY: lparam は呼び出しの間生きている `CHARRANGE`。
        unsafe {
            SendMessageW(
                edit,
                EM_EXSETSEL,
                None,
                Some(LPARAM(&raw const range as isize)),
            );
        }
    }

    fn selection(edit: HWND) -> CHARRANGE {
        let mut range = CHARRANGE::default();
        // SAFETY: lparam は書き先の `CHARRANGE`。
        unsafe {
            SendMessageW(
                edit,
                EM_EXGETSEL,
                None,
                Some(LPARAM(&raw mut range as isize)),
            );
        }
        range
    }

    /// RichEdit の DLL を一度だけ読み込み、解放しない（作ったコントロールが使う）。
    fn rich_edit_loaded() -> bool {
        static LOADED: std::sync::OnceLock<Option<zwcore::dl::Object>> = std::sync::OnceLock::new();
        LOADED
            .get_or_init(|| zwcore::dl::Object::open("Msftedit.dll").ok())
            .is_some()
    }

    /// キャレットを出させない（RichEdit はフォーカスを得るたびに `ShowCaret` し、隠す数は積み上がる）。
    unsafe extern "system" fn edit_hook(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let Some(original) = EDITS.with(|edits| edits.borrow().get(&(hwnd.0 as isize)).copied())
        else {
            return LRESULT(0);
        };
        let result = original.call(hwnd, message, wparam, lparam);
        if matches!(
            message,
            WM_SETFOCUS
                | WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_LBUTTONDBLCLK
                | WM_MOUSEMOVE
                | WM_KEYDOWN
                | WM_KEYUP
                | WM_CHAR
                | EM_SETSEL
                | EM_EXSETSEL
        ) {
            // SAFETY: 自分のコントロールのキャレットを隠すだけ。
            let _ = unsafe { HideCaret(Some(hwnd)) };
        }
        result
    }

    unsafe extern "system" fn hook(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let key = hwnd.0 as isize;
        let Some((original, edits)) = PARENTS.with(|parents| {
            parents
                .borrow()
                .get(&key)
                .map(|parent| (parent.original, parent.edits.clone()))
        }) else {
            return LRESULT(0);
        };
        if message == WM_NOTIFY {
            // SAFETY: `WM_NOTIFY` の lparam は `NMHDR` で始まる構造体。
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            let from = header.hwndFrom.0 as isize;
            // バッジの画像だけが選択されたら選択を解除する。
            if edits.contains(&from) && header.code == EN_SELCHANGE {
                // SAFETY: `EN_SELCHANGE` の通知は `SELCHANGE`。
                let change = unsafe { &*(lparam.0 as *const SELCHANGE) };
                let (min, max) = (change.chrg.cpMin, change.chrg.cpMax);
                if change.seltyp == SEL_OBJECT && max - min == 1 {
                    // 空の選択範囲の通知は `SEL_EMPTY` なので戻って来ない。
                    select(
                        HWND(from as *mut core::ffi::c_void),
                        CHARRANGE {
                            cpMin: min,
                            cpMax: min,
                        },
                    );
                }
                return LRESULT(0);
            }
            if edits.contains(&from) && header.code == EN_MSGFILTER {
                // SAFETY: `EN_MSGFILTER` の通知は `MSGFILTER`。
                let filter = unsafe { &*(lparam.0 as *const MSGFILTER) };
                let edit = HWND(from as *mut core::ffi::c_void);
                match filter.msg {
                    WM_RBUTTONUP => {
                        context_menu(hwnd, edit);
                        return LRESULT(0);
                    }
                    WM_KEYDOWN | WM_KEYUP if filter.wParam.0 == VK_ESCAPE => {
                        return LRESULT(1);
                    }
                    _ => {}
                }
            }
        }
        // コントロールが Esc で送ったものだけを握りつぶす（フォーカスだけで判断すると、文字を選択した後に ✕ で閉じなくなる）。
        // SAFETY: フォーカスのあるウィンドウと Esc キーの状態を取得するだけ。
        if message == WM_CLOSE
            && edits.contains(&(unsafe { GetFocus() }.0 as isize))
            && unsafe { GetKeyState(VK_ESCAPE as i32) } < 0
        {
            return LRESULT(0);
        }
        original.call(hwnd, message, wparam, lparam)
    }

    fn context_menu(parent: HWND, edit: HWND) {
        // SAFETY: 自分のウィンドウに対する Win32 のメニュー（`TPM_RETURNCMD`）。
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, COMMAND_COPY, w!("コピー(&C)"));
            let _ = AppendMenuW(menu, MF_STRING, COMMAND_SELECT_ALL, w!("すべて選択(&A)"));
            let mut at = POINT::default();
            let _ = GetCursorPos(&mut at);
            let picked = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                at.x,
                at.y,
                Some(0),
                parent,
                None,
            );
            let _ = DestroyMenu(menu);
            match picked.0 as usize {
                COMMAND_COPY => {
                    let range = selection(edit);
                    if range.cpMin == range.cpMax {
                        select_all(edit);
                        SendMessageW(edit, WM_COPY, None, None);
                        select(edit, range);
                    } else {
                        SendMessageW(edit, WM_COPY, None, None);
                    }
                }
                COMMAND_SELECT_ALL => select_all(edit),
                _ => {}
            }
        }
    }

    fn select_all(edit: HWND) {
        select(
            edit,
            CHARRANGE {
                cpMin: 0,
                cpMax: -1,
            },
        );
    }

    pub struct Inner {
        parent: HWND,
        edit: HWND,
        colour: COLORREF,
        dim: COLORREF,
        marks: Vec<(String, COLORREF)>,
        twips: i32,
        dpi: f64,
        mono: bool,
        face: Option<crate::ui::text::font::NativeFace>,
    }

    fn colorref([r, g, b]: [u8; 3]) -> COLORREF {
        COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
    }

    impl Inner {
        pub fn open(window: &Arc<Window>, scroll: bool) -> Option<Self> {
            let parent = win32::hwnd(window)?;
            let mut style = WS_CHILD
                | WS_CLIPSIBLINGS
                | WINDOW_STYLE(ES_MULTILINE as u32)
                | WINDOW_STYLE(ES_READONLY as u32)
                | WINDOW_STYLE(ES_NOHIDESEL as u32)
                | WINDOW_STYLE(ES_NOOLEDRAGDROP);
            if scroll {
                style |= WS_VSCROLL | WINDOW_STYLE(ES_AUTOVSCROLL as u32);
            }
            if !rich_edit_loaded() {
                return None;
            }
            // SAFETY: 有効な親ウィンドウに枠無しの子ウィンドウとして作る。
            let edit = unsafe {
                let instance = GetModuleHandleW(PCWSTR::null()).ok()?;
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("RICHEDIT50W"),
                    w!(""),
                    style,
                    0,
                    0,
                    0,
                    0,
                    Some(parent),
                    Some(HMENU::default()),
                    Some(instance.into()),
                    None,
                )
                .ok()?
            };
            // SAFETY: コントロールへの設定。IME のフォントの自動切替を切り、右クリックとキー入力の通知を受け取る。
            unsafe {
                let options = SendMessageW(edit, EM_GETLANGOPTIONS, None, None).0;
                let options = options & !((IMF_AUTOFONT | IMF_DUALFONT) as isize);
                SendMessageW(edit, EM_SETLANGOPTIONS, None, Some(LPARAM(options)));
                SendMessageW(
                    edit,
                    EM_SETEVENTMASK,
                    None,
                    Some(LPARAM(
                        (ENM_MOUSEEVENTS | ENM_KEYEVENTS | ENM_SELCHANGE) as isize,
                    )),
                );
            }
            // キャレットを隠すため。元のプロシージャは `Drop` で戻す。
            let edit_original = win32::subclass(edit, edit_hook);
            EDITS.with(|edits| {
                edits.borrow_mut().insert(edit.0 as isize, edit_original);
            });
            PARENTS.with(|parents| {
                let mut parents = parents.borrow_mut();
                match parents.entry(parent.0 as isize) {
                    std::collections::hash_map::Entry::Occupied(mut found) => {
                        found.get_mut().edits.push(edit.0 as isize);
                    }
                    std::collections::hash_map::Entry::Vacant(empty) => {
                        empty.insert(Parent {
                            original: win32::subclass(parent, hook),
                            edits: vec![edit.0 as isize],
                        });
                    }
                }
            });
            Some(Self {
                parent,
                edit,
                colour: COLORREF(0),
                dim: COLORREF(0x0080_8080),
                marks: Vec::new(),
                twips: 180,
                dpi: 96.0,
                mono: false,
                face: None,
            })
        }

        pub fn style(&mut self, window: &Arc<Window>, shown: &super::Shown) {
            self.colour = colorref(shown.colour);
            self.dim = colorref(shown.dim);
            self.marks = shown
                .marks
                .iter()
                .map(|(label, c)| (label.clone(), colorref(*c)))
                .collect();
            self.twips = (shown.font_pt * 20.0).round() as i32;
            self.dpi = window.scale_factor() * 96.0;
            self.mono = shown.mono;
            self.face = shown.face.clone();
            // SAFETY: コントロールへの設定。
            unsafe {
                SendMessageW(
                    self.edit,
                    EM_SETBKGNDCOLOR,
                    Some(WPARAM(0)),
                    Some(LPARAM(colorref(shown.back).0 as isize)),
                );
                let _ = SetWindowTheme(
                    self.edit,
                    if shown.dark {
                        w!("DarkMode_Explorer")
                    } else {
                        w!("Explorer")
                    },
                    PCWSTR::null(),
                );
            }
        }

        pub fn set_text(&mut self, shown: &super::Shown) {
            let flat = super::flatten(shown, "\r");
            let wide = wide(&flat.text);
            // SAFETY: 終端付きの UTF-16 と、コントロールへの書式。再描画は止めてまとめて行う。
            unsafe {
                SendMessageW(self.edit, WM_SETREDRAW, Some(WPARAM(0)), None);
                // 読み取り専用のコントロールは置換を拒むので、画像を入れる間だけ書き込み可能にする。
                SendMessageW(self.edit, EM_SETREADONLY, Some(WPARAM(0)), None);
                let _ = SetWindowTextW(self.edit, PCWSTR(wide.as_ptr()));
                let mut format = self.format(self.colour);
                SendMessageW(
                    self.edit,
                    EM_SETCHARFORMAT,
                    Some(WPARAM(SCF_ALL as usize)),
                    Some(LPARAM(&raw mut format as isize)),
                );
                let mut dim = self.format(self.dim);
                for &(from, len) in &flat.headings {
                    let range = CHARRANGE {
                        cpMin: from as i32,
                        cpMax: (from + len) as i32,
                    };
                    select(self.edit, range);
                    SendMessageW(
                        self.edit,
                        EM_SETCHARFORMAT,
                        Some(WPARAM(SCF_SELECTION as usize)),
                        Some(LPARAM(&raw mut dim as isize)),
                    );
                }
                // 囲みの中はバッジの画像に置き換える。後ろから行う（前からだと文字位置がずれる）。
                for (&(from, len), (label, colour)) in
                    flat.marks.iter().zip(self.marks.iter()).rev()
                {
                    let range = CHARRANGE {
                        cpMin: from as i32,
                        cpMax: (from + len) as i32,
                    };
                    select(self.edit, range);
                    self.insert_badge(label, *colour);
                }
                let (at, bottom) = if shown.tail { (-1, true) } else { (0, false) };
                let home = CHARRANGE {
                    cpMin: at,
                    cpMax: at,
                };
                select(self.edit, home);
                SendMessageW(self.edit, EM_SETREADONLY, Some(WPARAM(1)), None);
                SendMessageW(self.edit, WM_SETREDRAW, Some(WPARAM(1)), None);
                if bottom {
                    SendMessageW(self.edit, WM_VSCROLL, Some(WPARAM(SB_BOTTOM)), None);
                }
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.edit), None, true);
            }
        }

        pub fn append(&mut self, more: &str) {
            let wide = wide(&more.replace('\n', "\r"));
            // SAFETY: 自分のコントロールへの要求。文字列は終端付きの UTF-16。
            unsafe {
                let bottom = self.at_bottom();
                let first = SendMessageW(self.edit, EM_GETFIRSTVISIBLELINE, None, None).0;
                let was = selection(self.edit);
                SendMessageW(self.edit, WM_SETREDRAW, Some(WPARAM(0)), None);
                SendMessageW(self.edit, EM_SETREADONLY, Some(WPARAM(0)), None);
                let end = CHARRANGE {
                    cpMin: -1,
                    cpMax: -1,
                };
                select(self.edit, end);
                let mut format = self.format(self.colour);
                SendMessageW(
                    self.edit,
                    EM_SETCHARFORMAT,
                    Some(WPARAM(SCF_SELECTION as usize)),
                    Some(LPARAM(&raw mut format as isize)),
                );
                SendMessageW(
                    self.edit,
                    EM_REPLACESEL,
                    Some(WPARAM(0)),
                    Some(LPARAM(wide.as_ptr() as isize)),
                );
                select(self.edit, was);
                SendMessageW(self.edit, EM_SETREADONLY, Some(WPARAM(1)), None);
                if bottom {
                    SendMessageW(self.edit, WM_VSCROLL, Some(WPARAM(SB_BOTTOM)), None);
                } else {
                    // 選択を戻すと選択位置までスクロールされるので、見ていた行へ戻す。
                    let now = SendMessageW(self.edit, EM_GETFIRSTVISIBLELINE, None, None).0;
                    SendMessageW(
                        self.edit,
                        EM_LINESCROLL,
                        Some(WPARAM(0)),
                        Some(LPARAM(first - now)),
                    );
                }
                SendMessageW(self.edit, WM_SETREDRAW, Some(WPARAM(1)), None);
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(self.edit), None, true);
            }
        }

        fn at_bottom(&self) -> bool {
            use windows::Win32::UI::WindowsAndMessaging::{
                GetScrollInfo, SB_VERT, SCROLLINFO, SIF_ALL,
            };
            let mut info = SCROLLINFO {
                cbSize: size_of::<SCROLLINFO>() as u32,
                fMask: SIF_ALL,
                ..Default::default()
            };
            // SAFETY: 自分のコントロールのスクロールバーの情報を取得するだけ。
            if unsafe { GetScrollInfo(self.edit, SB_VERT, &raw mut info) }.is_err() {
                return true;
            }
            info.nPage == 0 || info.nPos + info.nPage as i32 >= info.nMax
        }

        fn insert_badge(&self, label: &str, colour: COLORREF) {
            use windows::Win32::Foundation::{RECT, SIZE};
            use windows::Win32::Graphics::Gdi::{
                ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
                CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_PITCH,
                DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC,
                DeleteObject, DrawTextW, FW_BOLD, FillRect, GetTextExtentPoint32W,
                OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TA_BASELINE,
                TRANSPARENT,
            };
            use windows::Win32::UI::Controls::RichEdit::{
                EM_INSERTIMAGE, RICHEDIT_IMAGE_PARAMETERS,
            };
            use windows::Win32::UI::Shell::SHCreateMemStream;

            let badge = super::badge(f64::from(self.twips) / 20.0 * self.dpi / 72.0);
            let small = badge.small;
            let height = badge.height.round().max(2.0) as i32;
            let pad = badge.pad.round() as i32;
            let radius = badge.radius;
            let units: Vec<u16> = label.encode_utf16().collect();
            // SAFETY: 自分で作った GDI オブジェクトを、作った順に使って捨てる。
            let (width, coverage) = unsafe {
                let hdc = CreateCompatibleDC(None);
                if hdc.is_invalid() {
                    return;
                }
                let font = CreateFontW(
                    -(small.round() as i32),
                    0,
                    0,
                    0,
                    FW_BOLD.0 as i32,
                    0,
                    0,
                    0,
                    windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    ANTIALIASED_QUALITY,
                    DEFAULT_PITCH.0.into(),
                    w!("Yu Gothic UI"),
                );
                let old_font = SelectObject(hdc, font.into());
                let mut extent = SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &units, &mut extent);
                let width = extent.cx + pad * 2;
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
                let Ok(bitmap) =
                    CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                else {
                    SelectObject(hdc, old_font);
                    let _ = DeleteObject(font.into());
                    let _ = DeleteDC(hdc);
                    return;
                };
                let old_bitmap = SelectObject(hdc, bitmap.into());
                let whole = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };
                let black = CreateSolidBrush(COLORREF(0));
                let _ = FillRect(hdc, &whole, black);
                let _ = DeleteObject(black.into());
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(0x00ff_ffff));
                let mut text_rect = whole;
                let mut text: Vec<u16> = units.clone();
                DrawTextW(
                    hdc,
                    &mut text,
                    &mut text_rect,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
                let pixels = (width * height) as usize;
                let coverage: Vec<u8> = if bits.is_null() {
                    vec![0; pixels]
                } else {
                    std::slice::from_raw_parts(bits.cast::<u8>(), pixels * 4)
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|px| px[2])
                        .collect()
                };
                SelectObject(hdc, old_bitmap);
                SelectObject(hdc, old_font);
                let _ = DeleteObject(font.into());
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(hdc);
                (width, coverage)
            };
            let (r, g, b) = (
                (colour.0 & 0xff) as u8,
                ((colour.0 >> 8) & 0xff) as u8,
                ((colour.0 >> 16) & 0xff) as u8,
            );
            let inside = |x: i32, y: i32| -> f64 {
                let (cx, cy) = (
                    (f64::from(x) + 0.5).clamp(radius, f64::from(width) - radius),
                    (f64::from(y) + 0.5).clamp(radius, f64::from(height) - radius),
                );
                let d =
                    ((f64::from(x) + 0.5 - cx).powi(2) + (f64::from(y) + 0.5 - cy).powi(2)).sqrt();
                (1.0 - (d - radius + 0.5)).clamp(0.0, 1.0)
            };
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for y in 0..height {
                for x in 0..width {
                    let alpha = inside(x, y);
                    let ink = f64::from(coverage[(y * width + x) as usize]) / 255.0;
                    let mix = |c: u8| (f64::from(c) * (1.0 - ink) + 255.0 * ink).round() as u8;
                    rgba.extend_from_slice(&[
                        mix(r),
                        mix(g),
                        mix(b),
                        (alpha * 255.0).round() as u8,
                    ]);
                }
            }
            let mut file = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut file, width as u32, height as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let Ok(mut writer) = encoder.write_header() else {
                    return;
                };
                if writer.write_image_data(&rgba).is_err() {
                    return;
                }
            }
            // SAFETY: メモリのストリームをコントロールへ渡す。渡した後はこちらで手放す。
            unsafe {
                let Some(stream) = SHCreateMemStream(Some(&file)) else {
                    return;
                };
                let himetric = |px: i32| (f64::from(px) * 2540.0 / self.dpi).round() as i32;
                // 代替テキストは必要（無いと `E_INVALIDARG`）。
                let alternate = wide(label);
                let mut params = RICHEDIT_IMAGE_PARAMETERS {
                    xWidth: himetric(width),
                    yHeight: himetric(height),
                    Ascent: himetric((f64::from(height) / 2.0 + badge.middle).round() as i32),
                    Type: TA_BASELINE.0 as i32,
                    pwszAlternateText: PCWSTR(alternate.as_ptr()),
                    pIStream: std::mem::ManuallyDrop::new(Some(stream)),
                };
                let done = SendMessageW(
                    self.edit,
                    EM_INSERTIMAGE,
                    None,
                    Some(LPARAM(&raw mut params as isize)),
                );
                if done.0 != 0 {
                    tracing::info!("番組説明: バッジの画像を挿入できない: {:#x}", done.0);
                }
                drop(std::mem::ManuallyDrop::into_inner(params.pIStream));
            }
        }

        fn format(&self, colour: COLORREF) -> CHARFORMATW {
            let mut format = CHARFORMATW {
                cbSize: size_of::<CHARFORMATW>() as u32,
                dwMask: CFM_FACE | CFM_SIZE | CFM_COLOR | CFM_CHARSET,
                yHeight: self.twips,
                crTextColor: colour,
                bCharSet: windows::Win32::Graphics::Gdi::DEFAULT_CHARSET,
                ..Default::default()
            };
            // 等幅は MS ゴシック（Consolas には仮名が無い）。
            let face = if self.mono {
                "MS Gothic"
            } else {
                self.face
                    .as_ref()
                    .map_or("Yu Gothic UI", |face| face.family.as_str())
            };
            if !self.mono {
                format.dwMask |= CFM_BOLD;
                if self.face.as_ref().is_some_and(|face| face.weight >= 600) {
                    format.dwEffects |= CFE_BOLD;
                }
            }
            for (slot, unit) in format.szFaceName.iter_mut().zip(face.encode_utf16()) {
                *slot = unit;
            }
            format
        }

        pub fn place(&mut self, (x, y, w, h): (i32, i32, i32, i32)) {
            // SAFETY: 自分の子ウィンドウを動かすだけ。
            unsafe {
                let _ = SetWindowPos(
                    self.edit,
                    None,
                    x,
                    y,
                    w.max(1),
                    h.max(1),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        pub fn reveal(&mut self) {
            // SAFETY: 自分の子ウィンドウ。
            unsafe {
                let _ = ShowWindow(self.edit, SW_SHOWNA);
            }
        }

        pub fn hide(&mut self) {
            // SAFETY: 自分の子ウィンドウ。
            unsafe {
                let _ = ShowWindow(self.edit, SW_HIDE);
            }
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            let restore = PARENTS.with(|parents| {
                let mut parents = parents.borrow_mut();
                let key = self.parent.0 as isize;
                let found = parents.get_mut(&key)?;
                found.edits.retain(|&edit| edit != self.edit.0 as isize);
                if found.edits.is_empty() {
                    parents.remove(&key).map(|parent| parent.original)
                } else {
                    None
                }
            });
            let edit_taken = EDITS.with(|edits| edits.borrow_mut().remove(&(self.edit.0 as isize)));
            if let Some(original) = restore {
                original.restore(self.parent);
            }
            if let Some(original) = edit_taken {
                original.restore(self.edit);
            }
            // SAFETY: 自分で作った子ウィンドウを破棄する。
            let _ = unsafe { DestroyWindow(self.edit) };
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {

    use std::sync::Arc;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAttributedStringAttachmentConveniences, NSAttributedStringNSStringDrawing, NSBezierPath,
        NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage,
        NSScrollView, NSTextAttachment, NSTextView, NSView,
    };
    use objc2_foundation::{
        NSAttributedString, NSDictionary, NSPoint, NSRange, NSRect, NSSize, NSString,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    pub struct Inner {
        parent: Retained<NSView>,
        scroll: Retained<NSScrollView>,
        text: Retained<NSTextView>,
        scale: f64,
        /// `setString` で色は消えるので入れ直すたびに付け直す。
        colour: [u8; 3],
        dim: [u8; 3],
        marks: Vec<(String, [u8; 3])>,
        points: f64,
    }

    fn badge_image(label: &str, colour: [u8; 3], badge: &super::Badge) -> Retained<NSImage> {
        let (pad, radius) = (badge.pad, badge.radius);
        let font = NSFont::boldSystemFontOfSize(badge.small);
        let white = ns_colour([255, 255, 255]);
        let keys: [&NSString; 2] = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let values: [Retained<AnyObject>; 2] = [
            Retained::into_super(Retained::into_super(font)),
            Retained::into_super(Retained::into_super(white)),
        ];
        let attributes = NSDictionary::from_retained_objects(&keys, &values);
        // SAFETY: 属性の辞書は上でビルドした、キーと型の合ったもの。
        let text = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(label),
                Some(&attributes),
            )
        };
        let measured = text.size();
        let size = NSSize::new((measured.width + pad * 2.0).ceil(), badge.height.ceil());
        let fill = ns_colour(colour);
        let handler = block2::RcBlock::new(move |rect: NSRect| -> Bool {
            fill.setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
            let at = NSPoint::new(
                rect.origin.x + (rect.size.width - measured.width) / 2.0,
                rect.origin.y + (rect.size.height - measured.height) / 2.0,
            );
            text.drawAtPoint(at);
            Bool::YES
        });
        NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler)
    }

    fn ns_colour([r, g, b]: [u8; 3]) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
            1.0,
        )
    }

    impl Inner {
        pub fn open(window: &Arc<Window>, scroller: bool) -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            let handle = window.window_handle().ok()?;
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return None;
            };
            // SAFETY: winit の有効なウィンドウの NSView。
            let parent: Retained<NSView> =
                unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>())? };
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
            let scroll = NSScrollView::initWithFrame(mtm.alloc(), frame);
            let text = NSTextView::initWithFrame(mtm.alloc(), frame);
            text.setEditable(false);
            text.setSelectable(true);
            text.setRichText(true);
            text.setVerticallyResizable(true);
            text.setHorizontallyResizable(false);
            text.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
            scroll.setHasVerticalScroller(scroller);
            scroll.setDrawsBackground(true);
            let document: &NSView = &text;
            scroll.setDocumentView(Some(document));
            scroll.setHidden(true);
            parent.addSubview(&scroll);
            Some(Self {
                parent,
                scroll,
                text,
                scale: window.scale_factor(),
                colour: [0; 3],
                dim: [0; 3],
                marks: Vec::new(),
                points: 12.0,
            })
        }

        pub fn style(&mut self, window: &Arc<Window>, shown: &super::Shown) {
            self.scale = window.scale_factor();
            self.colour = shown.colour;
            self.dim = shown.dim;
            self.marks = shown.marks.clone();
            let points = f64::from(shown.font_pt) / 0.75;
            self.points = points;
            unsafe {
                let font = if shown.mono {
                    NSFont::userFixedPitchFontOfSize(points)
                } else {
                    shown.face.as_ref().and_then(|face| {
                        NSFont::fontWithName_size(&NSString::from_str(&face.postscript), points)
                    })
                }
                .unwrap_or_else(|| NSFont::systemFontOfSize(points));
                self.text.setFont(Some(&font));
                let (fore, back) = (ns_colour(shown.colour), ns_colour(shown.back));
                self.text.setTextColor(Some(&fore));
                self.text.setBackgroundColor(&back);
                self.scroll.setBackgroundColor(&back);
                let appearance = NSAppearance::appearanceNamed(if shown.dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                });
                self.scroll.setAppearance(appearance.as_deref());
            }
        }

        pub fn set_text(&mut self, shown: &super::Shown) {
            let flat = super::flatten(shown, "\n");
            self.text.setString(&NSString::from_str(&flat.text));
            let whole = NSRange::new(0, flat.units);
            self.text
                .setTextColor_range(Some(&ns_colour(self.colour)), whole);
            let dim = ns_colour(self.dim);
            for (location, length) in flat.headings {
                self.text
                    .setTextColor_range(Some(&dim), NSRange::new(location, length));
            }
            // SAFETY: 範囲は上で数えた UTF-16 の単位で、文字列の中に収まる。
            unsafe {
                if let Some(storage) = self.text.textStorage() {
                    let badge = super::badge(self.points);
                    for (&(location, length), (label, colour)) in
                        flat.marks.iter().zip(self.marks.iter()).rev()
                    {
                        let image = badge_image(label, *colour, &badge);
                        let size = image.size();
                        let attachment = NSTextAttachment::new();
                        attachment.setImage(Some(&image));
                        attachment.setBounds(NSRect::new(
                            NSPoint::new(0.0, badge.middle - size.height / 2.0),
                            size,
                        ));
                        let piece = NSAttributedString::attributedStringWithAttachment(&attachment);
                        storage.replaceCharactersInRange_withAttributedString(
                            NSRange::new(location, length),
                            &piece,
                        );
                    }
                }
            }
            let at = if shown.tail { flat.units } else { 0 };
            self.text.scrollRangeToVisible(NSRange::new(at, 0));
        }

        pub fn append(&mut self, more: &str) {
            let bottom = self.at_bottom();
            // SAFETY: 自分のコントロールのテキスト。範囲は末尾の幅ゼロ。
            unsafe {
                let Some(storage) = self.text.textStorage() else {
                    return;
                };
                let end = storage.length();
                storage.replaceCharactersInRange_withString(
                    NSRange::new(end, 0),
                    &NSString::from_str(more),
                );
                if bottom {
                    self.text
                        .scrollRangeToVisible(NSRange::new(storage.length(), 0));
                }
            }
        }

        fn at_bottom(&self) -> bool {
            let visible = self.scroll.contentView().bounds();
            let document = self.text.frame();
            visible.origin.y + visible.size.height >= document.size.height - 2.0
        }

        pub fn place(&mut self, (x, y, w, h): (i32, i32, i32, i32)) {
            let scale = self.scale.max(0.01);
            let (x, y, w, h) = (
                f64::from(x) / scale,
                f64::from(y) / scale,
                f64::from(w) / scale,
                f64::from(h) / scale,
            );
            // winit の NSView は flipped ではないので下から測り直す。
            let flipped = self.parent.isFlipped();
            let top = if flipped {
                y
            } else {
                let height = self.parent.frame().size.height;
                height - y - h
            };
            unsafe {
                self.scroll
                    .setFrame(NSRect::new(NSPoint::new(x, top), NSSize::new(w, h)));
                let content = self.scroll.contentSize();
                self.text
                    .setFrameSize(NSSize::new(content.width, content.height));
                self.text.setMinSize(NSSize::new(0.0, content.height));
                self.text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
                if let Some(container) = self.text.textContainer() {
                    container.setContainerSize(NSSize::new(content.width, f64::MAX));
                    container.setWidthTracksTextView(true);
                }
            }
        }

        pub fn reveal(&mut self) {
            self.scroll.setHidden(false);
        }

        pub fn hide(&mut self) {
            self.scroll.setHidden(true);
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            self.scroll.removeFromSuperview();
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use std::sync::Arc;

    use winit::window::Window;

    pub struct Inner;

    impl Inner {
        pub fn open(_window: &Arc<Window>, _scroll: bool) -> Option<Self> {
            None
        }
        pub fn style(&mut self, _window: &Arc<Window>, _shown: &super::Shown) {}
        pub fn set_text(&mut self, _shown: &super::Shown) {}
        pub fn append(&mut self, _more: &str) {}
        pub fn place(&mut self, _rect: (i32, i32, i32, i32)) {}
        pub fn reveal(&mut self) {}
        pub fn hide(&mut self) {}
    }
}
