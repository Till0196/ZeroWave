// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `ui` hook：メニューと設定のページを取得する。インスタンスはスレッドごとに一つ。クラッシュしたら以後は触らない。

use wasmtime::TypedFunc;

use crate::abi;
use crate::host::Made;
use crate::ui::{Ctx, Event, MenuItem, Page, Reply};

pub struct UiInstance {
    name: String,
    made: Made,
    poisoned: bool,
    menu: TypedFunc<u32, u32>,
    pick: TypedFunc<(u32, u32), ()>,
    page: TypedFunc<(u32, u32), u32>,
    event: TypedFunc<(u32, u32, u32), u32>,
}

impl std::fmt::Debug for UiInstance {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("UiInstance")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl UiInstance {
    pub(super) fn new(name: String, mut made: Made) -> Result<Self, String> {
        Ok(Self {
            menu: made.func(abi::export::UI_MENU)?,
            pick: made.func(abi::export::UI_PICK)?,
            page: made.func(abi::export::UI_PAGE)?,
            event: made.func(abi::export::UI_EVENT)?,
            name,
            made,
            poisoned: false,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn put(&mut self, bytes: Vec<u8>) -> u32 {
        self.made.store.data_mut().put_buf(bytes)
    }

    fn finish(&mut self, given: &[u32], answer: Option<u32>) -> Vec<u8> {
        let state = self.made.store.data_mut();
        for &buf in given {
            state.bufs.take(buf);
        }
        answer
            .and_then(|answer| state.bufs.take(answer))
            .unwrap_or_default()
    }

    fn failed(&mut self, what: &str, error: impl std::fmt::Display) -> String {
        self.poisoned = true;
        format!("{}: {what}: {error}", self.name)
    }

    fn check(&self) -> Result<(), String> {
        if self.poisoned {
            Err(format!("{} はクラッシュしたので呼ばない", self.name))
        } else {
            Ok(())
        }
    }

    pub fn menu(&mut self, ctx: &Ctx) -> Result<Vec<MenuItem>, String> {
        self.check()?;
        let ctx = self.put(crate::ui::to_json(ctx));
        let result = self.menu.call(self.made.armed(), ctx);
        let answer = match result {
            Ok(answer) => answer,
            Err(error) => {
                self.finish(&[ctx], None);
                return Err(self.failed(abi::export::UI_MENU, error));
            }
        };
        let bytes = self.finish(&[ctx], Some(answer));
        crate::ui::from_json(&bytes)
            .map_err(|why| format!("{}: メニューが読めない: {why}", self.name))
    }

    pub fn pick(&mut self, ctx: &Ctx, id: &str) -> Result<(), String> {
        self.check()?;
        let ctx = self.put(crate::ui::to_json(ctx));
        let id = self.put(id.as_bytes().to_vec());
        let result = self.pick.call(self.made.armed(), (ctx, id));
        self.finish(&[ctx, id], None);
        result.map_err(|error| self.failed(abi::export::UI_PICK, error))
    }

    pub fn page(&mut self, page: &str, ctx: &Ctx) -> Result<Page, String> {
        self.check()?;
        let page = self.put(page.as_bytes().to_vec());
        let ctx = self.put(crate::ui::to_json(ctx));
        let result = self.page.call(self.made.armed(), (page, ctx));
        let answer = match result {
            Ok(answer) => answer,
            Err(error) => {
                self.finish(&[page, ctx], None);
                return Err(self.failed(abi::export::UI_PAGE, error));
            }
        };
        let bytes = self.finish(&[page, ctx], Some(answer));
        crate::ui::from_json(&bytes)
            .map_err(|why| format!("{}: ページが読めない: {why}", self.name))
    }

    pub fn event(&mut self, page: &str, ctx: &Ctx, event: &Event) -> Result<Reply, String> {
        self.check()?;
        let page = self.put(page.as_bytes().to_vec());
        let ctx = self.put(crate::ui::to_json(ctx));
        let event = self.put(crate::ui::to_json(event));
        let result = self.event.call(self.made.armed(), (page, ctx, event));
        let answer = match result {
            Ok(answer) => answer,
            Err(error) => {
                self.finish(&[page, ctx, event], None);
                return Err(self.failed(abi::export::UI_EVENT, error));
            }
        };
        let bytes = self.finish(&[page, ctx, event], Some(answer));
        crate::ui::from_json(&bytes).map_err(|why| format!("{}: 応答が読めない: {why}", self.name))
    }
}
