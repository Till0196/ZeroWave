// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインを書く側の SDK。native にビルドすればそのままテストできる。

pub mod abi;
mod buf;
pub mod host;
mod layer;
mod log;
mod ui;
mod video;

pub use buf::Buf;
pub use layer::{Layer, Painted};
pub use log::{Level, info, warn};
pub use ui::Ui;
pub use video::Video;

/// 変えたら [`export_stream!`] の中の文字列も。
pub const ABI: u32 = crate::abi::VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Unknown,
    Ts,
    Tlv,
}

impl Container {
    pub fn of(code: u32) -> Self {
        match code {
            1 => Self::Ts,
            2 => Self::Tlv,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub container: Container,
    pub service: Option<u16>,
}

/// 一つのスレッドからしか触られず、選局のたびに作られる。
pub trait Stream: 'static {
    fn new() -> Self;
    /// `Err` ならこのインスタンスは外される。
    fn start(&mut self, info: StreamInfo) -> Result<(), String>;
    /// そのまま通すなら `output_buf(buf)`。
    fn input(&mut self, buf: Buf);
    fn stop(&mut self) {}
}

pub fn output(bytes: &[u8]) {
    abi::output_bytes(bytes);
}

pub fn output_buf(buf: &Buf) {
    abi::output_buf(buf);
}

/// `order` は通す順（小さいほど先、既定 0）。
#[macro_export]
macro_rules! export_stream {
    ($stream:ty, name = $name:literal, version = $version:literal) => {
        $crate::export_stream!($stream, name = $name, version = $version, config = "[]");
    };
    ($stream:ty, name = $name:literal, version = $version:literal, config = $config:literal) => {
        $crate::export_stream!(
            $stream,
            name = $name,
            version = $version,
            order = 0,
            config = $config
        );
    };
    ($stream:ty, name = $name:literal, version = $version:literal, order = $order:literal, config = $config:literal) => {
        $crate::export_stream!(
            $stream,
            name = $name,
            version = $version,
            order = $order,
            hooks = "[\"stream\"]",
            config = $config
        );
    };
    ($stream:ty, name = $name:literal, version = $version:literal, order = $order:literal, hooks = $hooks:literal, config = $config:literal) => {
        $crate::export_stream!(@manifest $stream, ::std::concat!(
            "{\"abi\":1,\"name\":\"",
            $name,
            "\",\"version\":\"",
            $version,
            "\",\"hooks\":",
            $hooks,
            ",\"order\":",
            $order,
            ",\"config\":",
            $config,
            "}"
        ));
    };
    // `hooks` に `"ui"` が入り、`ui` の宣言を添える。
    ($stream:ty, name = $name:literal, version = $version:literal, order = $order:literal, ui = $ui:ty, ui_manifest = $ui_manifest:literal, config = $config:literal) => {
        $crate::export_stream!(@manifest $stream, ::std::concat!(
            "{\"abi\":1,\"name\":\"",
            $name,
            "\",\"version\":\"",
            $version,
            "\",\"hooks\":[\"stream\",\"ui\"],\"order\":",
            $order,
            ",\"ui\":",
            $ui_manifest,
            ",\"config\":",
            $config,
            "}"
        ));
        $crate::export_ui!($ui);
    };
    (@manifest $stream:ty, $text:expr) => {
        const _: () = {
            #[doc(hidden)]
            pub const MANIFEST: &str = $text;

            $crate::export_common!(MANIFEST);

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_stream_new() -> u32 {
                $crate::plug::abi::slab::put(::std::boxed::Box::new(
                    <$stream as $crate::plug::Stream>::new(),
                ))
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_stream_start(handle: u32, container: u32, service: u32) -> i32 {
                $crate::plug::abi::slab::with::<$stream, i32>(handle, |it| {
                    let info = $crate::plug::StreamInfo {
                        container: $crate::plug::Container::of(container),
                        service: (service != 0).then_some(service as u16),
                    };
                    match <$stream as $crate::plug::Stream>::start(it, info) {
                        Ok(()) => 0,
                        Err(why) => {
                            $crate::plug::warn(&why);
                            1
                        }
                    }
                })
                .unwrap_or(-1)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_stream_input(handle: u32, buf: u32) -> i32 {
                $crate::plug::abi::slab::with::<$stream, i32>(handle, |it| {
                    <$stream as $crate::plug::Stream>::input(it, $crate::plug::Buf::borrowed(buf));
                    0
                })
                .unwrap_or(-1)
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_stream_stop(handle: u32) {
                let _ = $crate::plug::abi::slab::with::<$stream, ()>(handle, |it| {
                    <$stream as $crate::plug::Stream>::stop(it)
                });
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_stream_drop(handle: u32) {
                $crate::plug::abi::slab::take::<$stream>(handle);
            }
        };
    };
}
