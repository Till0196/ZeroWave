// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 共有オブジェクトを名前で開く。開発ヘッダーもリンク時の依存も要らず、無いことは実行時に分かる。

use std::ffi::{CString, c_char, c_int, c_void};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing(pub String);

impl std::fmt::Display for Missing {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

impl std::error::Error for Missing {}

#[derive(Debug)]
pub struct Object(*mut c_void);

// SAFETY: handle はスレッドに紐づかず、閉じるのは `Drop` の一度だけ。
unsafe impl Send for Object {}
// SAFETY: `open` のあと handle を通して書くことは無い。
unsafe impl Sync for Object {}

impl Object {
    pub fn open(soname: &str) -> Result<Self, Missing> {
        if soname.contains('\0') {
            return Err(Missing(format!("{soname}: 名前に NUL")));
        }
        #[cfg(unix)]
        let handle = {
            let name =
                CString::new(soname).map_err(|_| Missing(format!("{soname}: 名前に NUL")))?;
            // SAFETY: `name` は呼び出しより長生きする正しい C 文字列。
            unsafe { dlopen(name.as_ptr(), RTLD_NOW | RTLD_LOCAL) }
        };
        #[cfg(windows)]
        let handle = load_library(soname);
        if handle.is_null() {
            return Err(Missing(format!("{soname} を開けません")));
        }
        Ok(Self(handle))
    }

    /// 順に試して最初に開けたもの（`libGLESv2.so.2` と `.so` のように名前が環境で異なる）。
    pub fn open_any(sonames: &[&str]) -> Result<Self, Missing> {
        for soname in sonames {
            if let Ok(object) = Self::open(soname) {
                return Ok(object);
            }
        }
        Err(Missing(format!("{} を開けません", sonames.join(" / "))))
    }

    /// `name` は NUL 終端（`b"cuInit\0"`）。
    /// # Safety
    /// `T` は本当にそのシンボルの型でなければならない。
    pub unsafe fn symbol<T: Copy>(&self, name: &[u8]) -> Result<T, Missing> {
        debug_assert_eq!(name.last(), Some(&0));
        // SAFETY: 呼び出し側が保証するとおり。
        unsafe { self.find_terminated(name) }
            .ok_or_else(|| Missing(format!("{} がありません", name_of(name))))
    }

    /// # Safety
    /// [`Object::symbol`] と同じ。
    pub unsafe fn symbol_named<T: Copy>(&self, name: &str) -> Result<T, Missing> {
        // SAFETY: 呼び出し側が保証するとおり。
        unsafe { self.find(name) }.ok_or_else(|| Missing(format!("{name} がありません")))
    }

    /// 無いことが普通のシンボル（拡張のエントリーポイント）のために。
    ///
    /// # Safety
    /// [`Object::symbol`] と同じ。
    pub unsafe fn find<T: Copy>(&self, name: &str) -> Option<T> {
        let text = CString::new(name).ok()?;
        // SAFETY: 呼び出し側が保証するとおり。`text` は終端されている。
        unsafe { self.find_terminated(text.as_bytes_with_nul()) }
    }

    /// 名前の無いエクスポートを序数で取得する。
    ///
    /// # Safety
    /// [`Object::symbol`] と同じ。
    #[cfg(windows)]
    pub unsafe fn ordinal<T: Copy>(&self, number: u16) -> Option<T> {
        debug_assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
        // SAFETY: 序数は下位 16 ビットだけの値として渡す（`MAKEINTRESOURCE`）。
        let pointer =
            unsafe { GetProcAddress(self.0, std::ptr::without_provenance(usize::from(number))) };
        if pointer.is_null() {
            return None;
        }
        // SAFETY: 非 null で、`T` は同じ大きさの関数ポインタ。
        Some(unsafe { *(&raw const pointer).cast::<T>() })
    }

    /// # Safety
    /// `name` は NUL 終端、`T` はそのシンボルの型。
    unsafe fn find_terminated<T: Copy>(&self, name: &[u8]) -> Option<T> {
        debug_assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
        // SAFETY: `name` は NUL 終端、handle は成功した読み込みのもの。
        #[cfg(unix)]
        let pointer = unsafe { dlsym(self.0, name.as_ptr().cast()) };
        #[cfg(windows)]
        let pointer = unsafe { GetProcAddress(self.0, name.as_ptr().cast()) };
        if pointer.is_null() {
            return None;
        }
        // SAFETY: 非 null で、`T` は同じ大きさの関数ポインタ。
        Some(unsafe { *(&raw const pointer).cast::<T>() })
    }
}

fn name_of(name: &[u8]) -> String {
    String::from_utf8_lossy(name.strip_suffix(&[0]).unwrap_or(name)).into_owned()
}

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: 開いた handle を一度だけ閉じる。
        #[cfg(unix)]
        unsafe {
            dlclose(self.0)
        };
        #[cfg(windows)]
        unsafe {
            FreeLibrary(self.0)
        };
    }
}

#[cfg(unix)]
const RTLD_NOW: c_int = 2;
#[cfg(unix)]
const RTLD_LOCAL: c_int = 0;

#[cfg(unix)]
unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

/// 名前だけなら実行ファイルのディレクトリと System32 だけを探す（カレントディレクトリや PATH の同名 DLL を
/// 読まない）。
#[cfg(windows)]
pub(crate) fn load_library(name: &str) -> *mut c_void {
    const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: u32 = 0x0000_0100;
    const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: u32 = 0x0000_1000;
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let flags = if std::path::Path::new(name).is_absolute() {
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
    } else {
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
    };
    // SAFETY: `wide` は呼び出しより長生きする NUL 終端の UTF-16。
    unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), flags) }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_is_not_there_is_said_to_be_missing() {
        assert!(Object::open("libzerowave-no-such-thing.so.0").is_err());
        let missing = Object::open_any(&["libzerowave-no-such-thing.so.0", "nor-this.so"])
            .expect_err("開けてしまった");
        assert!(missing.0.contains("nor-this.so"), "{missing}");
    }
}
