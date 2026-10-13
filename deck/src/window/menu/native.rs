// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! [`super`] の項目を OS のメニューに変換する。メニューバーは macOS、右クリックメニューは macOS と Windows。

#[cfg(target_os = "macos")]
use std::collections::HashMap;

use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, ContextMenu, Menu, MenuItem, PredefinedMenuItem, Submenu};

#[cfg(target_os = "macos")]
use super::command_of;
use super::{EMPTY, Entry, Filling, Group, Item, Picked, entries_of, id_of};

pub struct Popup {
    menu: Option<Menu>,
    picks: Vec<Picked>,
    groups: Vec<(Submenu, Group, usize)>,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    root: isize,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    open: Option<isize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Updated {
    Same,
    Redrawn,
    /// ポップアップのウィンドウは自分ではサイズを変えないので、呼ぶ側が再計算させる（`MN_SIZEWINDOW`）。
    Resized,
}

impl Popup {
    pub const fn none() -> Self {
        Self {
            menu: None,
            picks: Vec::new(),
            groups: Vec::new(),
            root: 0,
            open: None,
        }
    }
}

impl std::fmt::Debug for Popup {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Popup").finish_non_exhaustive()
    }
}

pub struct Prepared {
    menu: Menu,
    at: Option<(f64, f64)>,
    dark: bool,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Prepared").finish_non_exhaustive()
    }
}

impl Prepared {
    pub fn present(&self, window: &winit::window::Window) -> bool {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = window.window_handle() else {
            return false;
        };
        let position = self
            .at
            .map(|(x, y)| muda::dpi::Position::Logical(muda::dpi::LogicalPosition::new(x, y)));
        match handle.as_raw() {
            #[cfg(target_os = "windows")]
            RawWindowHandle::Win32(handle) => {
                app_mode(if self.dark { FORCE_DARK } else { FORCE_LIGHT });
                // SAFETY: winit の有効なウィンドウの HWND。閉じるまで戻らない。
                unsafe {
                    self.menu
                        .show_context_menu_for_hwnd(handle.hwnd.get(), position)
                };
                app_mode(ALLOW_DARK);
                true
            }
            #[cfg(target_os = "macos")]
            RawWindowHandle::AppKit(handle) => {
                let _ = self.dark;
                // SAFETY: winit の有効なウィンドウの NSView。
                unsafe {
                    self.menu
                        .show_context_menu_for_nsview(handle.ns_view.as_ptr(), position)
                };
                true
            }
            _ => false,
        }
    }
}

impl Popup {
    pub fn prepare(&mut self, filling: &Filling, dark: bool) -> Option<Prepared> {
        self.prepare_items(super::CONTEXT, filling, dark, None)
    }

    pub fn prepare_items(
        &mut self,
        items: &[Item],
        filling: &Filling,
        dark: bool,
        at: Option<(f64, f64)>,
    ) -> Option<Prepared> {
        let menu = Menu::new();
        self.picks.clear();
        self.groups.clear();
        if let Err(why) = fill_with(&menu, items, filling, &mut self.picks, &mut self.groups) {
            tracing::warn!("メニューを作成できない（OS のメニュー）: {why}");
            return None;
        }
        #[cfg(target_os = "windows")]
        {
            self.root = menu.hpopupmenu();
        }
        Some(Prepared { menu, at, dark })
    }

    pub fn keep(&mut self, prepared: Prepared) {
        self.menu = Some(prepared.menu);
    }

    /// `WM_INITMENUPOPUP` の `HMENU` はルートに結び付いたほうのハンドルで、muda は公開しないのでルートから `GetSubMenu` で取る。
    #[cfg(target_os = "windows")]
    pub fn refill(&mut self, hmenu: isize, filling: &Filling) -> bool {
        if self.root == 0 {
            return false;
        }
        let root = self.root;
        let Some((submenu, group)) = self
            .groups
            .iter()
            .find(|(_, _, at)| submenu_at(root, *at) == hmenu)
            .map(|(submenu, group, _)| (submenu.clone(), *group))
        else {
            return false;
        };
        self.open = Some(hmenu);
        while submenu.remove_at(0).is_some() {}
        if let Err(why) = place_with(
            &submenu,
            &entries_of(group, filling),
            &mut self.picks,
            "pop",
            self.root,
        ) {
            tracing::warn!("メニューを作成できない（OS のメニュー）: {why}");
        }
        true
    }

    /// 開いている項目の文字列とチェックをその場で書き換える。項目の数や形が変わっていれば触らない。
    pub fn update_open(&mut self, filling: &Filling) -> Updated {
        let mut changed = false;
        let mut resized = false;
        let groups: Vec<(Submenu, Group, usize)> = self.groups.clone();
        for (submenu, group, at) in &groups {
            let entries = entries_of(*group, filling);
            let synced = sync_with(submenu, &entries);
            if synced.is_err() && self.is_open(*at) {
                while submenu.remove_at(0).is_some() {}
                if let Err(why) = place_with(submenu, &entries, &mut self.picks, "pop", self.root) {
                    tracing::warn!("メニューを作成できない（OS のメニュー）: {why}");
                }
                resized = true;
            }
            if let Ok(touched) = synced {
                changed |= touched;
            }
        }
        if resized {
            Updated::Resized
        } else if changed {
            Updated::Redrawn
        } else {
            Updated::Same
        }
    }

    #[cfg(target_os = "windows")]
    pub fn closed(&mut self, hmenu: isize) {
        if self.open == Some(hmenu) {
            self.open = None;
        }
    }

    #[cfg(target_os = "windows")]
    fn is_open(&self, at: usize) -> bool {
        let Some(open) = self.open else {
            return false;
        };
        self.root != 0 && submenu_at(self.root, at) == open
    }

    #[cfg(not(target_os = "windows"))]
    fn is_open(&self, _at: usize) -> bool {
        false
    }

    pub fn picked(&self, id: &str) -> Option<Picked> {
        let number = id.strip_prefix("pop:")?;
        number
            .parse::<usize>()
            .ok()
            .and_then(|at| self.picks.get(at).copied())
    }
}

fn fill_with(
    into: &Menu,
    items: &[Item],
    filling: &Filling,
    picks: &mut Vec<Picked>,
    groups: &mut Vec<(Submenu, Group, usize)>,
) -> Result<(), muda::Error> {
    #[cfg(target_os = "windows")]
    let root = into.hpopupmenu();
    #[cfg(not(target_os = "windows"))]
    let root = 0isize;
    if items.is_empty() {
        return into.append(&MenuItem::new(EMPTY, false, None));
    }
    for item in items {
        if hidden_here(item) {
            continue;
        }
        match item {
            Item::Separator => into.append(&PredefinedMenuItem::separator())?,
            Item::Command {
                label,
                command,
                key,
                enabled,
                toggle,
            } => {
                let accelerator = key.and_then(|key| key.parse::<Accelerator>().ok());
                if *toggle {
                    into.append(&CheckMenuItem::with_id(
                        id_of(*command),
                        *label,
                        *enabled,
                        filling.is_on(*command),
                        accelerator,
                    ))?;
                } else {
                    into.append(&MenuItem::with_id(
                        id_of(*command),
                        *label,
                        *enabled,
                        accelerator,
                    ))?;
                }
            }
            Item::Group {
                label,
                group,
                enabled,
            } => {
                let submenu = Submenu::new(*label, *enabled);
                place_with(&submenu, &entries_of(*group, filling), picks, "pop", root)?;
                let at = into.items().len();
                into.append(&submenu)?;
                groups.push((submenu, *group, at));
            }
        }
    }
    Ok(())
}

/// Windows のメニューを明暗に合わせる（uxtheme.dll の番号 135・136）。閉じたら `AllowDark` に戻す
/// （固定したままだと winit が OS の明暗を追えなくなる）。
#[cfg(target_os = "windows")]
fn app_mode(mode: i32) {
    type SetMode = extern "system" fn(i32) -> i32;
    type Flush = extern "system" fn();
    static ENTRIES: std::sync::OnceLock<Option<(zwcore::dl::Object, SetMode, Flush)>> =
        std::sync::OnceLock::new();
    let entries = ENTRIES.get_or_init(|| {
        let uxtheme = zwcore::dl::Object::open("uxtheme.dll").ok()?;
        // SAFETY: 序数 135・136 の関数の型は uxtheme のもので、引数は列挙値一つか無し。
        let (set_mode, flush) = unsafe { (uxtheme.ordinal(135)?, uxtheme.ordinal(136)?) };
        Some((uxtheme, set_mode, flush))
    });
    if let Some((_, set_mode, flush)) = entries {
        set_mode(mode);
        flush();
    }
}

#[cfg(target_os = "windows")]
const ALLOW_DARK: i32 = 1;
#[cfg(target_os = "windows")]
const FORCE_DARK: i32 = 2;
#[cfg(target_os = "windows")]
const FORCE_LIGHT: i32 = 3;

#[cfg(target_os = "macos")]
pub struct Native {
    _menu: Menu,
    groups: HashMap<Group, Submenu>,
    toggles: Vec<(CheckMenuItem, super::Command)>,
    picks: Vec<Picked>,
    filled: Option<Filling>,
}

#[cfg(target_os = "macos")]
impl std::fmt::Debug for Native {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Native").finish_non_exhaustive()
    }
}

#[cfg(target_os = "macos")]
impl Native {
    pub fn attach() -> Option<Self> {
        let menu = Menu::new();
        let app = Submenu::new("ZeroWave", true);
        let about = MenuItem::with_id(
            id_of(super::Command::About),
            "ZeroWave について",
            true,
            None,
        );
        let settings = MenuItem::with_id(
            id_of(super::Command::Settings),
            "設定...",
            true,
            "CmdOrCtrl+,".parse::<Accelerator>().ok(),
        );
        if let Err(why) = app.append_items(&[
            &about,
            &PredefinedMenuItem::separator(),
            &settings,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(Some("ZeroWave を隠す")),
            &PredefinedMenuItem::hide_others(Some("ほかを隠す")),
            &PredefinedMenuItem::show_all(Some("すべてを表示")),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(Some("ZeroWave を終了")),
        ]) {
            tracing::warn!("メニューを作成できない（アプリ）: {why}");
            return None;
        }
        if let Err(why) = menu.append(&app) {
            tracing::warn!("メニューを作成できない（アプリ）: {why}");
            return None;
        }
        let mut groups = HashMap::new();
        let mut toggles = Vec::new();
        for (label, items) in super::BAR {
            let kept: Vec<Item> = items
                .iter()
                .filter(|item| {
                    !matches!(
                        item,
                        Item::Command {
                            command: super::Command::About | super::Command::Settings,
                            ..
                        }
                    )
                })
                .copied()
                .collect();
            let mut items: Vec<Item> = Vec::with_capacity(kept.len());
            for item in kept {
                if matches!(item, Item::Separator)
                    && matches!(items.last(), None | Some(Item::Separator))
                {
                    continue;
                }
                items.push(item);
            }
            if matches!(items.last(), Some(Item::Separator)) {
                items.pop();
            }
            let submenu = Submenu::new(*label, true);
            if let Err(why) = fill(&submenu, &items, &mut groups, &mut toggles) {
                tracing::warn!("メニューを作成できない（{label}）: {why}");
                return None;
            }
            if let Err(why) = menu.append(&submenu) {
                tracing::warn!("メニューを作成できない（{label}）: {why}");
                return None;
            }
        }
        menu.init_for_nsapp();
        Some(Self {
            _menu: menu,
            groups,
            toggles,
            picks: Vec::new(),
            filled: None,
        })
    }

    pub fn refill(&mut self, filling: &Filling) {
        if self.filled.as_ref() == Some(filling) {
            return;
        }
        self.filled = Some(filling.clone());
        self.picks.clear();
        for (item, command) in &self.toggles {
            item.set_checked(filling.is_on(*command));
        }
        for (group, submenu) in &self.groups {
            while submenu.remove_at(0).is_some() {}
            let entries = entries_of(*group, filling);
            if let Err(why) = place_with(submenu, &entries, &mut self.picks, "bar", 0) {
                tracing::warn!("メニューを再構築できない（{group:?}）: {why}");
            }
        }
    }

    pub fn picked(&self, id: &str) -> Option<Picked> {
        if let Some(number) = id.strip_prefix("bar:") {
            return number
                .parse::<usize>()
                .ok()
                .and_then(|at| self.picks.get(at).copied());
        }
        command_of(id).map(Picked::Command)
    }
}

fn sync_with(into: &Submenu, entries: &[Entry]) -> Result<bool, ()> {
    use muda::MenuItemKind;
    let items = into.items();
    let entries: Vec<&Entry> = entries
        .iter()
        .filter(|entry| !matches!(entry, Entry::Break))
        .collect();
    if entries.is_empty() {
        return if items.len() == 1 { Ok(false) } else { Err(()) };
    }
    if items.len() != entries.len() {
        return Err(());
    }
    let mut changed = false;
    for (item, entry) in items.iter().zip(entries) {
        match (item, entry) {
            (
                MenuItemKind::MenuItem(item),
                Entry::Pick {
                    label,
                    chosen: None,
                    ..
                },
            ) => {
                if item.text() != *label {
                    item.set_text(label);
                    changed = true;
                }
            }
            (
                MenuItemKind::Check(item),
                Entry::Pick {
                    label,
                    chosen: Some(chosen),
                    ..
                },
            ) => {
                if item.text() != *label {
                    item.set_text(label);
                    changed = true;
                }
                if item.is_checked() != *chosen {
                    item.set_checked(*chosen);
                    changed = true;
                }
            }
            (
                MenuItemKind::Submenu(below),
                Entry::Sub {
                    label,
                    entries: inside,
                },
            ) => {
                if below.text() != *label {
                    below.set_text(label);
                    changed = true;
                }
                changed |= sync_with(below, inside)?;
            }
            (MenuItemKind::Predefined(_), Entry::Separator) => {}
            (MenuItemKind::MenuItem(item), Entry::Off { label }) => {
                if item.text() != *label {
                    item.set_text(label);
                    changed = true;
                }
            }
            _ => return Err(()),
        }
    }
    Ok(changed)
}

/// `root` はルートの `HMENU`。muda の `Submenu` で見えるのは親に結び付いたほうのハンドルなので、ルートから辿って書く。
fn place_with(
    into: &Submenu,
    entries: &[Entry],
    picks: &mut Vec<Picked>,
    prefix: &str,
    root: isize,
) -> Result<(), muda::Error> {
    if entries.is_empty() {
        return into.append(&MenuItem::new(EMPTY, false, None));
    }
    let mut breaking = false;
    for entry in entries {
        match entry {
            Entry::Pick {
                label,
                chosen,
                picked,
                logo,
            } => {
                let id = format!("{prefix}:{}", picks.len());
                picks.push(*picked);
                match chosen {
                    Some(chosen) => {
                        into.append(&CheckMenuItem::with_id(id, label, true, *chosen, None))?;
                        if let Some(logo) = logo {
                            attach_logo(into, root, logo);
                        }
                    }
                    None => into.append(&MenuItem::with_id(id, label, true, None))?,
                }
            }
            Entry::Sub {
                label,
                entries: below,
            } => {
                let submenu = Submenu::new(label, true);
                place_with(&submenu, below, picks, prefix, root)?;
                into.append(&submenu)?;
            }
            Entry::Break => {
                breaking = true;
                continue;
            }
            Entry::Separator => into.append(&muda::PredefinedMenuItem::separator())?,
            Entry::Off { label } => into.append(&MenuItem::new(label, false, None))?,
        }
        if breaking {
            breaking = false;
            break_column(into, root);
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn attach_logo(into: &Submenu, root: isize, logo: &std::sync::Arc<zwcore::guide::Logo>) {
    use windows::Win32::Graphics::Gdi::HBITMAP;
    use windows::Win32::UI::WindowsAndMessaging::{
        HMENU, MENUITEMINFOW, MIIM_BITMAP, SetMenuItemInfoW,
    };
    let Some(id) = last_item_id(into) else {
        return;
    };
    let Some(bitmap) = logo_bitmap(logo) else {
        return;
    };
    let info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_BITMAP,
        hbmpItem: HBITMAP(bitmap as *mut core::ffi::c_void),
        ..Default::default()
    };
    // SAFETY: 表示中のメニューのルートと、その下の項目の ID。画像の欄だけを書く。
    unsafe {
        let _ = SetMenuItemInfoW(HMENU(root as *mut core::ffi::c_void), id, false, &info);
    }
}

/// 表示中のメニューのルート `root` の `at` 番目のサブメニュー（無ければ 0）。
#[cfg(target_os = "windows")]
fn submenu_at(root: isize, at: usize) -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{GetSubMenu, HMENU};
    // SAFETY: 無効なハンドルや位置なら null が返るだけ。
    unsafe { GetSubMenu(HMENU(root as *mut core::ffi::c_void), at as i32) }.0 as isize
}

#[cfg(target_os = "windows")]
fn last_item_id(into: &Submenu) -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemID, HMENU};
    let last = into.items().len().checked_sub(1)?;
    let hmenu = HMENU(into.hpopupmenu() as *mut core::ffi::c_void);
    // SAFETY: muda が持つ生きた HMENU と、その項目の位置。
    let id = unsafe { GetMenuItemID(hmenu, last as i32) };
    (id != u32::MAX).then_some(id)
}

#[cfg(not(target_os = "windows"))]
fn attach_logo(_into: &Submenu, _root: isize, _logo: &std::sync::Arc<zwcore::guide::Logo>) {}

#[cfg(target_os = "windows")]
fn logo_bitmap(logo: &std::sync::Arc<zwcore::guide::Logo>) -> Option<isize> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::SM_CYMENUCHECK;
    // ロゴのコピーも持つ（アドレスの再利用を避けるため）。画像は項目が指したままのことがあるので消さない。
    type Made = HashMap<(usize, u32), (std::sync::Arc<zwcore::guide::Logo>, Option<isize>)>;
    thread_local! {
        static MADE: RefCell<Made> = RefCell::new(HashMap::new());
    }
    let height = crate::window::win32::system_metric(SM_CYMENUCHECK).max(12) as u32;
    let key = (std::sync::Arc::as_ptr(logo) as usize, height);
    if let Some(made) = MADE.with(|made| {
        made.borrow()
            .get(&key)
            .filter(|(same, _)| std::sync::Arc::ptr_eq(same, logo))
            .map(|(_, made)| *made)
    }) {
        return made;
    }
    let made = (|| {
        let picture = logo.rgba()?;
        let width = (height * 16).div_ceil(9);
        let scaled = resize(&picture, width, height);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: 大きさを指定した BITMAPINFO と、書き戻されるピクセルのバッファ。
        let bitmap =
            unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        if bits.is_null() {
            return None;
        }
        let pixels = (width * height) as usize;
        // SAFETY: CreateDIBSection が width×height×4 バイトを用意した。
        let out = unsafe { std::slice::from_raw_parts_mut(bits as *mut u8, pixels * 4) };
        for (dst, src) in out
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(scaled.as_chunks::<4>().0)
        {
            let a = u32::from(src[3]);
            dst[0] = ((u32::from(src[2]) * a + 127) / 255) as u8;
            dst[1] = ((u32::from(src[1]) * a + 127) / 255) as u8;
            dst[2] = ((u32::from(src[0]) * a + 127) / 255) as u8;
            dst[3] = src[3];
        }
        Some(bitmap.0 as isize)
    })();
    MADE.with(|cache| {
        cache
            .borrow_mut()
            .insert(key, (std::sync::Arc::clone(logo), made));
    });
    made
}

#[cfg(target_os = "windows")]
fn resize(picture: &zwcore::image::Rgba, width: u32, height: u32) -> Vec<u8> {
    let (sw, sh) = (picture.width as usize, picture.height as usize);
    let mut out = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        let y0 = y * sh / height as usize;
        let y1 = ((y + 1) * sh / height as usize).max(y0 + 1).min(sh);
        for x in 0..width as usize {
            let x0 = x * sw / width as usize;
            let x1 = ((x + 1) * sw / width as usize).max(x0 + 1).min(sw);
            let mut sum = [0u32; 4];
            let mut n = 0;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let at = (sy * sw + sx) * 4;
                    for (total, &value) in sum.iter_mut().zip(&picture.rgba[at..at + 4]) {
                        *total += u32::from(value);
                    }
                    n += 1;
                }
            }
            let at = (y * width as usize + x) * 4;
            for c in 0..4 {
                out[at + c] = (sum[c] / n.max(1)) as u8;
            }
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn break_column(into: &Submenu, root: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMenuItemInfoW, HMENU, MENUITEMINFOW, MFT_MENUBREAK, MIIM_FTYPE, SetMenuItemInfoW,
    };
    let Some(id) = last_item_id(into) else {
        return;
    };
    let root = HMENU(root as *mut core::ffi::c_void);
    let mut info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_FTYPE,
        ..Default::default()
    };
    // SAFETY: 表示しているメニューのルートと項目の番号。型の欄だけを読み書きする。
    unsafe {
        if GetMenuItemInfoW(root, id, false, &mut info).is_ok() {
            info.fType |= MFT_MENUBREAK;
            let _ = SetMenuItemInfoW(root, id, false, &info);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn break_column(_into: &Submenu, _root: isize) {}

#[cfg(target_os = "macos")]
fn fill(
    into: &Submenu,
    items: &[Item],
    groups: &mut HashMap<Group, Submenu>,
    toggles: &mut Vec<(CheckMenuItem, super::Command)>,
) -> Result<(), muda::Error> {
    if items.is_empty() {
        return into.append(&MenuItem::new(EMPTY, false, None));
    }
    for item in items {
        if hidden_here(item) {
            continue;
        }
        match item {
            Item::Separator => into.append(&PredefinedMenuItem::separator())?,
            Item::Command {
                label,
                command,
                key,
                enabled,
                toggle,
            } => {
                let accelerator = key.and_then(|key| match key.parse::<Accelerator>() {
                    Ok(accelerator) => Some(accelerator),
                    Err(why) => {
                        tracing::warn!("ショートカットキーを解釈できない（{label}: {key}）: {why}");
                        None
                    }
                });
                if *toggle {
                    let item = CheckMenuItem::with_id(
                        id_of(*command),
                        *label,
                        *enabled,
                        false,
                        accelerator,
                    );
                    into.append(&item)?;
                    toggles.push((item, *command));
                } else {
                    into.append(&MenuItem::with_id(
                        id_of(*command),
                        *label,
                        *enabled,
                        accelerator,
                    ))?;
                }
            }
            Item::Group {
                label,
                group,
                enabled,
            } => {
                let submenu = Submenu::new(*label, *enabled);
                submenu.append(&MenuItem::new(EMPTY, false, None))?;
                into.append(&submenu)?;
                groups.insert(*group, submenu);
            }
        }
    }
    Ok(())
}

fn hidden_here(item: &Item) -> bool {
    cfg!(target_os = "macos")
        && matches!(
            item,
            Item::Command {
                command: super::Command::MenuBar,
                ..
            }
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// キーの表記が muda で解釈できること（できないとその項目だけ黙ってキーが利かなくなる）。
    #[test]
    fn every_key_is_one_the_menu_can_read() {
        for item in super::super::BAR
            .iter()
            .flat_map(|(_, items)| items.iter())
            .chain(super::super::CONTEXT.iter())
        {
            if let Item::Command {
                key: Some(key),
                label,
                ..
            } = item
            {
                assert!(
                    key.parse::<Accelerator>().is_ok(),
                    "{label} のキー {key} が読めない"
                );
            }
        }
    }
}
