// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 使い終えた大きなメモリーの後始末。

/// malloc のアリーナを二つに絞る（glibc は解放した分が断片化して OS へ返らない）。
/// スレッドが生まれる前に一度だけ呼ぶ。glibc でなければ何もしない。
pub fn few_arenas() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        const M_ARENA_MAX: std::ffi::c_int = -8;
        unsafe extern "C" {
            fn mallopt(param: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        // 環境変数で決めてあればそちらに従う。
        if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
            // SAFETY: 決まった名前と値を渡すだけで、どの状態から呼んでもよい。
            unsafe { mallopt(M_ARENA_MAX, 2) };
        }
    }
}

/// 解放したメモリーを OS へ返す（大量に使った後、メモリー 1 GB の環境でスワップアウトされないように）。
/// glibc でなければ何もしない。
pub fn give_back() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: 引数は残すバイト数だけで、どの状態から呼んでもよい。
        unsafe { malloc_trim(0) };
    }
}
