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

const GIVE_BACK_EVERY: std::time::Duration = std::time::Duration::from_secs(300);

/// 解放したメモリーを OS へ返す（大量に使った後、メモリー 1 GB の環境でスワップアウトされないように）。
/// glibc と Windows のプロセスの heap だけ。詰める間は heap を握るので、ほかのスレッド（映像も）の確保が止まる。
/// 大きな heap では数十〜数百 ms 掛かるので、[`GIVE_BACK_EVERY`] に一度まで。
pub fn give_back() {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    if last.is_some_and(|at| at.elapsed() < GIVE_BACK_EVERY) {
        return;
    }
    *last = Some(std::time::Instant::now());
    drop(last);
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetProcessHeap() -> isize;
            fn HeapCompact(heap: isize, flags: u32) -> usize;
        }
        // SAFETY: プロセスの heap を詰めるだけで、どの状態から呼んでもよい。
        unsafe { HeapCompact(GetProcessHeap(), 0) };
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: 引数は残すバイト数だけで、どの状態から呼んでもよい。
        unsafe { malloc_trim(0) };
    }
}
