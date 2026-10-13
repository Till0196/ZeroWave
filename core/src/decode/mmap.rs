// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `mmap` の宣言とマップした領域を一箇所に（V4L2 と VA-API）。違う型で二度宣言するとエラー無しに誤動作する。
//! `off_t` はポインターと同じ幅。`i64` にすると armhf で `EINVAL` になる。

use std::ffi::{c_int, c_void};
use std::io;

unsafe extern "C" {
    #[link_name = "mmap"]
    fn mmap_raw(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: isize,
    ) -> *mut c_void;
    #[link_name = "munmap"]
    fn munmap_raw(addr: *mut c_void, length: usize) -> c_int;
}

const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_SHARED: c_int = 1;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

/// ファイル記述子を共有でマップした領域。drop でアンマップする。
#[derive(Debug)]
pub struct Mapping {
    address: *mut c_void,
    length: usize,
    #[cfg_attr(not(all(feature = "v4l2", target_os = "linux")), allow(dead_code))]
    writable: bool,
}

// SAFETY: この構造体だけが持つ領域で、書くのは `&mut self` からだけ。
unsafe impl Send for Mapping {}

impl Mapping {
    /// `fd` の `offset` から `length` バイトをマップする。`offset` と `length` は記述子の所有者
    /// （ドライバー）が返した値を渡す。
    pub fn new(fd: c_int, offset: usize, length: usize, writable: bool) -> io::Result<Self> {
        let prot = if writable {
            PROT_READ | PROT_WRITE
        } else {
            PROT_READ
        };
        // SAFETY: 新しい領域をマップするだけで、既存の領域には触れない。
        let address = unsafe {
            mmap_raw(
                std::ptr::null_mut(),
                length,
                prot,
                MAP_SHARED,
                fd,
                offset as isize,
            )
        };
        if address == MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            address,
            length,
            writable,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        // SAFETY: `length` バイトの有効なマッピングで、アンマップは drop の時だけ。
        unsafe { std::slice::from_raw_parts(self.address.cast::<u8>(), self.length) }
    }

    /// 読み取り専用でマップした領域では空。
    #[cfg_attr(not(all(feature = "v4l2", target_os = "linux")), allow(dead_code))]
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        if !self.writable {
            return &mut [];
        }
        // SAFETY: 書き込み可能な `length` バイトの有効なマッピングで、`&mut self` が借用を一つに限る。
        unsafe { std::slice::from_raw_parts_mut(self.address.cast::<u8>(), self.length) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `mmap` が返したアドレスと長さで、アンマップは一度だけ。
        unsafe {
            munmap_raw(self.address, self.length);
        }
    }
}
