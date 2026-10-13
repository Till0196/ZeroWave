// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 本体との境界。wasm では import、native ではテスト用のローカルのスタブ（スレッドごとに持つ）。

#[cfg(target_arch = "wasm32")]
mod wasm {
    // 本体は渡されたアドレスと長さを確かめてコピーするだけで guest のメモリーへは書かないので、
    // どんな引数で呼んでも guest の側は壊れない。書く `buf_read` だけは unsafe のまま。
    #[link(wasm_import_module = "zw_buf_v1")]
    unsafe extern "C" {
        #[link_name = "len"]
        pub safe fn buf_len(buf: u32) -> u32;
        #[link_name = "read"]
        pub fn buf_read(buf: u32, from: u32, ptr: *mut u8, len: u32) -> u32;
        #[link_name = "create"]
        pub safe fn buf_create(ptr: *const u8, len: u32) -> u32;
        #[link_name = "drop"]
        pub safe fn buf_drop(buf: u32);
    }

    #[link(wasm_import_module = "zw_output_v1")]
    unsafe extern "C" {
        #[link_name = "write"]
        pub safe fn output_buf(buf: u32);
        #[link_name = "write_bytes"]
        pub safe fn output_bytes(ptr: *const u8, len: u32);
    }

    #[link(wasm_import_module = "zw_log_v1")]
    unsafe extern "C" {
        #[link_name = "log"]
        pub safe fn log(level: u32, ptr: *const u8, len: u32);
    }
}

#[cfg(target_arch = "wasm32")]
pub fn buf_len(buf: u32) -> usize {
    wasm::buf_len(buf) as usize
}

#[cfg(target_arch = "wasm32")]
pub fn buf_read(buf: u32, from: usize, into: &mut [u8]) -> usize {
    // SAFETY: 本体の import。`into` は生きている。
    unsafe { wasm::buf_read(buf, from as u32, into.as_mut_ptr(), into.len() as u32) as usize }
}

#[cfg(target_arch = "wasm32")]
pub fn buf_create(bytes: &[u8]) -> u32 {
    wasm::buf_create(bytes.as_ptr(), bytes.len() as u32)
}

#[cfg(target_arch = "wasm32")]
pub fn buf_drop(buf: u32) {
    wasm::buf_drop(buf)
}

#[cfg(target_arch = "wasm32")]
pub fn output_buf(buf: &crate::plug::Buf) {
    wasm::output_buf(buf.handle())
}

#[cfg(target_arch = "wasm32")]
pub fn output_bytes(bytes: &[u8]) {
    wasm::output_bytes(bytes.as_ptr(), bytes.len() as u32)
}

#[cfg(target_arch = "wasm32")]
pub fn log(level: u32, text: &str) {
    wasm::log(level, text.as_ptr(), text.len() as u32)
}

#[cfg(not(target_arch = "wasm32"))]
pub mod native {
    use std::cell::RefCell;

    use crate::slots::Slots;

    thread_local! {
        static BUFS: RefCell<Slots<Vec<u8>>> = const { RefCell::new(Slots::new()) };
        static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
        static LOGGED: RefCell<Vec<(u32, String)>> = const { RefCell::new(Vec::new()) };
    }

    pub fn buf_create(bytes: &[u8]) -> u32 {
        BUFS.with(|bufs| bufs.borrow_mut().put(bytes.to_vec()))
    }

    pub fn buf_len(buf: u32) -> usize {
        BUFS.with(|bufs| bufs.borrow().get(buf).map_or(0, Vec::len))
    }

    pub fn buf_read(buf: u32, from: usize, into: &mut [u8]) -> usize {
        BUFS.with(|bufs| {
            bufs.borrow()
                .get(buf)
                .map_or(0, |bytes| super::copy_from(bytes, from, into))
        })
    }

    pub fn buf_drop(buf: u32) {
        BUFS.with(|bufs| bufs.borrow_mut().take(buf));
    }

    pub fn output_buf(buf: &crate::plug::Buf) {
        output_bytes(&buf.to_vec());
    }

    pub fn output_bytes(bytes: &[u8]) {
        OUTPUT.with(|out| out.borrow_mut().extend_from_slice(bytes));
    }

    pub fn log(level: u32, text: &str) {
        LOGGED.with(|logged| logged.borrow_mut().push((level, text.to_owned())));
    }

    pub fn take_output() -> Vec<u8> {
        OUTPUT.with(|out| std::mem::take(&mut *out.borrow_mut()))
    }

    pub fn take_log() -> Vec<(u32, String)> {
        LOGGED.with(|logged| std::mem::take(&mut *logged.borrow_mut()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{buf_create, buf_drop, buf_len, buf_read, log, output_buf, output_bytes};

/// 本体と native のスタブが同じ読み方をするように共有する。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn copy_from(bytes: &[u8], from: usize, into: &mut [u8]) -> usize {
    let rest = bytes.get(from..).unwrap_or_default();
    let n = into.len().min(rest.len());
    into[..n].copy_from_slice(&rest[..n]);
    n
}

/// wasm は一つのスレッドなので、スレッドごとのテーブルで足りる。
pub mod slab {
    use std::any::Any;
    use std::cell::RefCell;

    use crate::slots::Slots;

    thread_local! {
        static SLAB: RefCell<Slots<Box<dyn Any>>> = const { RefCell::new(Slots::new()) };
    }

    pub fn put<T: 'static>(it: Box<T>) -> u32 {
        SLAB.with(|slab| slab.borrow_mut().put(it))
    }

    pub fn with<T: 'static, R>(handle: u32, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        SLAB.with(|slab| Some(f(slab.borrow_mut().get_mut(handle)?.downcast_mut::<T>()?)))
    }

    pub fn take<T: 'static>(handle: u32) -> Option<Box<T>> {
        SLAB.with(|slab| {
            let mut slab = slab.borrow_mut();
            let slot = slab.slot(handle)?;
            if !slot.as_ref()?.is::<T>() {
                return None;
            }
            slot.take()?.downcast::<T>().ok()
        })
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! export_common {
    ($manifest:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn zw_alloc(len: u32) -> *mut u8 {
            let mut bytes = ::std::vec::Vec::<u8>::with_capacity(len as usize);
            let ptr = bytes.as_mut_ptr();
            ::std::mem::forget(bytes);
            ptr
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn zw_free(ptr: *mut u8, len: u32) {
            if !ptr.is_null() {
                // SAFETY: `zw_alloc` が同じ長さで割り当てたもの。
                drop(unsafe { ::std::vec::Vec::from_raw_parts(ptr, 0, len as usize) });
            }
        }

        // 止めてあるプラグインの名前をコンパイルせずに読めるよう、custom section にも置く。
        #[cfg(target_arch = "wasm32")]
        #[unsafe(link_section = "zw_manifest")]
        #[used]
        static ZW_MANIFEST_SECTION: [u8; $manifest.len()] = {
            let text = $manifest.as_bytes();
            let mut out = [0u8; $manifest.len()];
            let mut at = 0;
            while at < text.len() {
                out[at] = text[at];
                at += 1;
            }
            out
        };

        #[unsafe(no_mangle)]
        pub extern "C" fn zw_manifest() -> u64 {
            let text: &'static str = $manifest;
            ((text.as_ptr() as u64) << 32) | text.len() as u64
        }
    };
}

pub use export_common;
