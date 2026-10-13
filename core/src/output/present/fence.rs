// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! GPU が出した命令を終えたかを、待たずに確かめる（EGL_KHR_fence_sync と、GLES のコンテキストで使うための
//! GL_OES_EGL_sync）。外部テクスチャで直接読んでいた DMA-BUF を、GPU が読み終えてからデコーダーへ返すために使う。

use std::ffi::c_void;

use super::egl::{self, Egl};
use super::gl::{self, Gl};

const EGL_SYNC_FENCE_KHR: egl::EGLenum = 0x30F9;
const EGL_SYNC_FLUSH_COMMANDS_BIT_KHR: egl::EGLint = 0x0001;
const EGL_CONDITION_SATISFIED_KHR: egl::EGLint = 0x30F6;

type Sync = *mut c_void;

/// 置いたフェンス。[`Fences::forget`] で一度だけ消す。
#[derive(Debug)]
pub struct Fence(Sync);

#[derive(Debug)]
pub struct Fences {
    display: egl::EGLDisplay,
    create: unsafe extern "C" fn(egl::EGLDisplay, egl::EGLenum, *const egl::EGLint) -> Sync,
    destroy: unsafe extern "C" fn(egl::EGLDisplay, Sync) -> egl::EGLBoolean,
    wait: unsafe extern "C" fn(egl::EGLDisplay, Sync, egl::EGLint, u64) -> egl::EGLint,
}

impl Fences {
    /// どちらかの拡張が無ければ `None`。
    pub fn open(egl: &Egl, gl: &Gl, display: egl::EGLDisplay) -> Option<Self> {
        let has = |list: String, name: &str| list.split_whitespace().any(|found| found == name);
        if !has(
            egl.query(display, egl::EGL_EXTENSIONS),
            "EGL_KHR_fence_sync",
        ) || !has(gl.string(gl::GL_EXTENSIONS), "GL_OES_EGL_sync")
        {
            return None;
        }
        Some(Self {
            display,
            create: egl.proc_address("eglCreateSyncKHR")?,
            destroy: egl.proc_address("eglDestroySyncKHR")?,
            wait: egl.proc_address("eglClientWaitSyncKHR")?,
        })
    }

    /// ここまでに出した命令の後ろにフェンスを置き、命令を GPU へ送る（送らないといつまでも終わらない）。
    pub fn put(&self) -> Option<Fence> {
        let attributes = [egl::EGL_NONE];
        // SAFETY: 有効なコンテキストのディスプレイで、属性は `EGL_NONE` で終わる。
        let sync = unsafe { (self.create)(self.display, EGL_SYNC_FENCE_KHR, attributes.as_ptr()) };
        if sync.is_null() {
            return None;
        }
        // SAFETY: 作ったばかりのフェンス。待たずに（0 ns）命令を送るだけ。
        unsafe { (self.wait)(self.display, sync, EGL_SYNC_FLUSH_COMMANDS_BIT_KHR, 0) };
        Some(Fence(sync))
    }

    /// 前の命令を GPU が終えたか（待たない）。
    pub fn passed(&self, fence: &Fence) -> bool {
        // SAFETY: まだ消していないフェンス。
        unsafe { (self.wait)(self.display, fence.0, 0, 0) == EGL_CONDITION_SATISFIED_KHR }
    }

    pub fn forget(&self, fence: Fence) {
        // SAFETY: このディスプレイで作り、消すのは一度だけ。
        unsafe { (self.destroy)(self.display, fence.0) };
    }
}
