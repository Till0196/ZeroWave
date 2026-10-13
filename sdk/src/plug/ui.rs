// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! メニューと設定のページを Rust で書く（[`crate::ui`]）。

use crate::ui::{Ctx, Event, MenuItem, Page, Reply};

/// モジュールに一つ（最初に呼ばれたときに作る）。
pub trait Ui: 'static {
    fn new() -> Self;
    fn menu(&mut self, ctx: &Ctx) -> Vec<MenuItem>;
    fn pick(&mut self, ctx: &Ctx, id: &str);
    fn page(&mut self, page: &str, ctx: &Ctx) -> Page;
    fn event(&mut self, page: &str, ctx: &Ctx, event: &Event) -> Reply;
}

/// マニフェストは出さない（`hooks` に `"ui"` を入れ、`ui` を添えること）。
#[macro_export]
macro_rules! export_ui {
    ($ui:ty) => {
        const _: () = {
            ::std::thread_local! {
                static UI: ::std::cell::RefCell<::std::option::Option<$ui>> =
                    const { ::std::cell::RefCell::new(::std::option::Option::None) };
            }

            fn with<R>(f: impl FnOnce(&mut $ui) -> R) -> R {
                UI.with(|ui| {
                    let mut ui = ui.borrow_mut();
                    f(ui.get_or_insert_with(<$ui as $crate::plug::Ui>::new))
                })
            }

            fn read<T: $crate::ui::Word>(buf: u32) -> T {
                $crate::ui::decode(&$crate::plug::Buf::borrowed(buf).to_vec())
            }

            fn text(buf: u32) -> ::std::string::String {
                ::std::string::String::from_utf8_lossy(&$crate::plug::Buf::borrowed(buf).to_vec())
                    .into_owned()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_ui_menu(ctx: u32) -> u32 {
                let ctx: $crate::ui::Ctx = read(ctx);
                let items = with(|ui| <$ui as $crate::plug::Ui>::menu(ui, &ctx));
                $crate::plug::Buf::of(&$crate::ui::to_json(&items)).into_handle()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_ui_pick(ctx: u32, id: u32) {
                let ctx: $crate::ui::Ctx = read(ctx);
                let id = text(id);
                with(|ui| <$ui as $crate::plug::Ui>::pick(ui, &ctx, &id));
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_ui_page(page: u32, ctx: u32) -> u32 {
                let page = text(page);
                let ctx: $crate::ui::Ctx = read(ctx);
                let page = with(|ui| <$ui as $crate::plug::Ui>::page(ui, &page, &ctx));
                $crate::plug::Buf::of(&$crate::ui::to_json(&page)).into_handle()
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn zw_ui_event(page: u32, ctx: u32, event: u32) -> u32 {
                let page = text(page);
                let ctx: $crate::ui::Ctx = read(ctx);
                let event: $crate::ui::Event = read(event);
                let reply = with(|ui| <$ui as $crate::plug::Ui>::event(ui, &page, &ctx, &event));
                $crate::plug::Buf::of(&$crate::ui::to_json(&reply)).into_handle()
            }
        };
    };
}
