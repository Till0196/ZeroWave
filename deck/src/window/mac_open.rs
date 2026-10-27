// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Finder の「このアプリケーションで開く」やドックへのドロップで渡されたファイルを受け取る。
//! winit のアプリケーションデリゲートは `application:openURLs:` を持たないので、イベントループを作ったあとに足す。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::Sender;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSString;

use crate::threads::link::{Act, ToStage};

static TO_STAGE: OnceLock<Sender<ToStage>> = OnceLock::new();

/// `EventLoop` を作ったあと、`run_app` の前に呼ぶ（起動時に渡されたファイルは起動の途中で届く）。
pub fn accept(to_stage: Sender<ToStage>) {
    let _ = TO_STAGE.set(to_stage);
    let Some(class) = AnyClass::get(c"WinitApplicationDelegate") else {
        tracing::warn!(
            "アプリケーションデリゲートが見つからない（Finder から渡されたファイルを開けない）"
        );
        return;
    };
    let open: unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject, *mut AnyObject) =
        open_urls;
    // SAFETY: 型の文字列は void (id self, SEL, id application, NSArray<NSURL *> *urls) と合っている。
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const AnyClass).cast_mut(),
            sel!(application:openURLs:),
            std::mem::transmute::<_, Imp>(open),
            c"v@:@@".as_ptr(),
        )
    };
    if !added.as_bool() {
        tracing::warn!("application:openURLs: を足せない");
    }
}

unsafe extern "C-unwind" fn open_urls(
    _delegate: &AnyObject,
    _: Sel,
    _application: *mut AnyObject,
    urls: *mut AnyObject,
) {
    let Some(urls) = (unsafe { urls.as_ref() }) else {
        return;
    };
    let count: usize = unsafe { msg_send![urls, count] };
    for index in 0..count {
        let url: *mut AnyObject = unsafe { msg_send![urls, objectAtIndex: index] };
        let Some(url) = (unsafe { url.as_ref() }) else {
            continue;
        };
        let path: Option<Retained<NSString>> = unsafe { msg_send![url, path] };
        let Some(path) = path else { continue };
        let path = PathBuf::from(path.to_string());
        if !crate::window::opening::playable(&path) {
            tracing::info!(
                "{} は TS でも MMT/TLV でもない（内容で判定）",
                path.display()
            );
            continue;
        }
        tracing::info!("Finder から開く: {}", path.display());
        if let Some(to_stage) = TO_STAGE.get() {
            let _ = to_stage.send(ToStage::Act(Act::Open(path)));
        }
    }
}
