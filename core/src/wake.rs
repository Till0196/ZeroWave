// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画面を再レンダリングさせるためのフック。中身は呼び出し側が決める。待たない。

use std::sync::Arc;

/// 何度呼んでも、何も起きなくてもよい。
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub fn nobody() -> Waker {
    Arc::new(|| {})
}
