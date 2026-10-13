// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! レイヤーのプラグインを Rust で書く。どのレイヤーでも同じ書き方。

use crate::layer::{Config, Event, Frame, Message, Ticked};

/// `None` は消えた、そうでなければ属性と RGBA（canvas にレンダリングしたなら空）。
pub type Painted = Option<(Frame, Vec<u8>)>;

/// 一つのインスタンスは一つのスレッドからしか触られない。
pub trait Layer: 'static {
    fn new(config: &Config) -> Self;
    fn message(&mut self, message: Message, bytes: &[u8]);
    fn tick(&mut self) -> Ticked {
        Ticked::default()
    }
    /// `None` は変わっていない。
    fn frame(&mut self) -> Option<Painted>;
    fn events(&mut self) -> (Vec<Event>, Vec<Vec<u8>>) {
        (Vec::new(), Vec::new())
    }
}

#[macro_export]
macro_rules! export_layer {
    ($layer:ty, name = $name:literal, version = $version:literal, layer = $manifest:literal) => {
        $crate::export_layer!(
            $layer,
            name = $name,
            version = $version,
            layer = $manifest,
            config = "[]"
        );
    };
    ($layer:ty, name = $name:literal, version = $version:literal, layer = $manifest:literal,
     config = $config:literal) => {
        $crate::export_layer!(
            $layer,
            name = $name,
            version = $version,
            layer = $manifest,
            config = $config,
            hooks = "[\"layer\"]"
        );
    };
    ($layer:ty, name = $name:literal, version = $version:literal, layer = $manifest:literal,
     config = $config:literal, hooks = $hooks:literal) => {
        const _: () = {
            #[doc(hidden)]
            pub const MANIFEST: &str = ::std::concat!(
                "{\"abi\":1,\"name\":\"",
                $name,
                "\",\"version\":\"",
                $version,
                "\",\"hooks\":",
                $hooks,
                ",\"layer\":",
                $manifest,
                ",\"config\":",
                $config,
                "}"
            );

            $crate::export_common!(MANIFEST);

            fn __zw_bytes(buf: u32) -> ::std::vec::Vec<u8> {
                if buf == 0 {
                    return ::std::vec::Vec::new();
                }
                $crate::plug::Buf::borrowed(buf).to_vec()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_layer_new(config: u32) -> u32 {
                let config: $crate::layer::Config =
                    $crate::layer::from_json(&__zw_bytes(config)).unwrap_or_default();
                $crate::plug::abi::slab::put(::std::boxed::Box::new(
                    <$layer as $crate::plug::Layer>::new(&config),
                ))
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_layer_message(handle: u32, json: u32, bytes: u32) {
                let Some(message) =
                    $crate::layer::from_json::<$crate::layer::Message>(&__zw_bytes(json))
                else {
                    return;
                };
                let bytes = __zw_bytes(bytes);
                let _ = $crate::plug::abi::slab::with::<$layer, ()>(handle, |it| {
                    <$layer as $crate::plug::Layer>::message(it, message, &bytes)
                });
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_layer_tick(handle: u32) -> u32 {
                let ticked = $crate::plug::abi::slab::with::<$layer, _>(handle, |it| {
                    <$layer as $crate::plug::Layer>::tick(it)
                })
                .unwrap_or_default();
                $crate::plug::Buf::of(&$crate::layer::to_json(&ticked)).into_handle()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_layer_frame(handle: u32) -> u32 {
                match $crate::plug::abi::slab::with::<$layer, _>(handle, |it| {
                    <$layer as $crate::plug::Layer>::frame(it)
                })
                .flatten()
                {
                    None => 0,
                    Some(None) => $crate::plug::Buf::of(&[]).into_handle(),
                    Some(Some((frame, rgba))) => {
                        $crate::plug::Buf::of(&$crate::layer::encode_frame(&frame, &rgba))
                            .into_handle()
                    }
                }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_layer_events(handle: u32) -> u32 {
                let (events, audios) = $crate::plug::abi::slab::with::<$layer, _>(handle, |it| {
                    <$layer as $crate::plug::Layer>::events(it)
                })
                .unwrap_or_default();
                if events.is_empty() {
                    return 0;
                }
                $crate::plug::Buf::of(&$crate::layer::encode_events(&events, &audios)).into_handle()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_layer_drop(handle: u32) {
                $crate::plug::abi::slab::take::<$layer>(handle);
            }
        };
    };
}
