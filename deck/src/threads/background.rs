// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 急がない裏の仕事（番組表の取得・統合・保存）のスレッドの優先度を下げる。遅い CPU では選局の直後に
//! 映像のデコードや描画と取り合い、フレームが間に合わなくなる。

/// 呼んだスレッドの優先度を下げる（CPU に加えて、Windows では I/O とメモリーも）。
pub fn lower() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
        };
        // SAFETY: 自分のスレッドの疑似ハンドル。
        if let Err(error) =
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }
        {
            tracing::debug!("スレッドの優先度を下げられない: {error}");
        }
    }
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: 引数を取るだけで、自分のスレッドにしか効かない。
        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
    }
    #[cfg(all(unix, not(target_vendor = "apple")))]
    {
        // Linux の nice はスレッドごと（`gettid` の ID で効く）。
        // SAFETY: 引数を取るだけで、自分のスレッドにしか効かない。
        unsafe {
            let id = libc::syscall(libc::SYS_gettid) as libc::id_t;
            libc::setpriority(libc::PRIO_PROCESS, id, 10);
        }
    }
}
