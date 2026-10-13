// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 映像のフィルターを Rust で書く。画素には触らない。

use crate::video::{Env, Graph};

/// 一つのインスタンスは一つのスレッドからしか触られない。
pub trait Video: 'static {
    fn new() -> Self;
    /// `Err` なら本体はこのフィルターを外して理由をログに出す。
    fn build(&mut self, env: &Env) -> Result<Graph, String>;

    /// 違えばビルドし直す。`None` は毎回ビルドし直す。レンダリングスレッドで呼ぶので軽い処理にすること。
    fn key(&mut self, env: &Env) -> Option<String> {
        let _ = env;
        None
    }

    /// 配置先が二つあるプラグインは、配置先に依らないイベントを映像のチェーンの側だけが受け取る。
    fn event(&mut self, event: &crate::video::Event) {
        let _ = event;
    }
}

#[macro_export]
macro_rules! export_video {
    ($video:ty, name = $name:literal, version = $version:literal, video = $manifest:literal) => {
        $crate::export_video!(
            $video,
            name = $name,
            version = $version,
            video = $manifest,
            config = "[]"
        );
    };
    ($video:ty, name = $name:literal, version = $version:literal, video = $manifest:literal,
     config = $config:literal) => {
        $crate::export_video!(@manifest $video, ::std::concat!(
            "{\"abi\":1,\"name\":\"",
            $name,
            "\",\"version\":\"",
            $version,
            "\",\"hooks\":[\"video\"],\"video\":",
            $manifest,
            ",\"config\":",
            $config,
            "}"
        ));
    };
    ($video:ty, name = $name:literal, version = $version:literal, video = $manifest:literal,
     ui = $ui:ty, ui_manifest = $ui_manifest:literal, config = $config:literal) => {
        $crate::export_video!(@manifest $video, ::std::concat!(
            "{\"abi\":1,\"name\":\"",
            $name,
            "\",\"version\":\"",
            $version,
            "\",\"hooks\":[\"video\",\"ui\"],\"video\":",
            $manifest,
            ",\"ui\":",
            $ui_manifest,
            ",\"config\":",
            $config,
            "}"
        ));
        $crate::export_ui!($ui);
    };
    (@manifest $video:ty, $text:expr) => {
        const _: () = {
            #[doc(hidden)]
            pub const MANIFEST: &str = $text;

            $crate::export_common!(MANIFEST);

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_video_new() -> u32 {
                $crate::plug::abi::slab::put(::std::boxed::Box::new(
                    <$video as $crate::plug::Video>::new(),
                ))
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_video_build(handle: u32, env: u32) -> u32 {
                let bytes = $crate::plug::Buf::borrowed(env).to_vec();
                let reply: ::std::result::Result<$crate::video::Graph, ::std::string::String> =
                    match $crate::video::from_json::<$crate::video::Env>(&bytes) {
                        Err(why) => Err(::std::format!("環境が読めない: {why}")),
                        Ok(env) => $crate::plug::abi::slab::with::<$video, _>(handle, |it| {
                            <$video as $crate::plug::Video>::build(it, &env)
                        })
                        .unwrap_or_else(|| Err(::std::string::String::from("インスタンスが無い"))),
                    };
                let reply = reply.and_then(|graph| {
                    for (name, blob) in &graph.blobs {
                        if !$crate::plug::host::video::put(name, &blob.0) {
                            return Err(::std::format!("blob {name} を渡せない"));
                        }
                    }
                    Ok(graph)
                });
                $crate::plug::Buf::of(&$crate::video::to_json(&reply)).into_handle()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_video_key(handle: u32, env: u32) -> u32 {
                let bytes = $crate::plug::Buf::borrowed(env).to_vec();
                let Ok(env) = $crate::video::from_json::<$crate::video::Env>(&bytes) else {
                    return 0;
                };
                $crate::plug::abi::slab::with::<$video, _>(handle, |it| {
                    <$video as $crate::plug::Video>::key(it, &env)
                })
                .flatten()
                .map_or(0, |key| $crate::plug::Buf::of(key.as_bytes()).into_handle())
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_video_event(handle: u32, event: u32) {
                let bytes = $crate::plug::Buf::borrowed(event).to_vec();
                if let Ok(event) = $crate::video::from_json::<$crate::video::Event>(&bytes) {
                    $crate::plug::abi::slab::with::<$video, _>(handle, |it| {
                        <$video as $crate::plug::Video>::event(it, &event)
                    });
                }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_video_drop(handle: u32) {
                $crate::plug::abi::slab::take::<$video>(handle);
            }
        };
    };
}
