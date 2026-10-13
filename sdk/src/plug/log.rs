// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use crate::plug::abi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info = 0,
    Warning = 1,
}

pub fn say(level: Level, text: &str) {
    abi::log(level as u32, text);
}

pub fn info(text: &str) {
    say(Level::Info, text);
}

pub fn warn(text: &str) {
    say(Level::Warning, text);
}
