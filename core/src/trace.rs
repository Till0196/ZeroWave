// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デバイスの応答を、指定されたときだけログに出す。`ZW_TRACE=1`/`all` で全部、`ZW_TRACE=v4l2,nvdec` で経路を選ぶ。
//! 失敗の報告はここを通さない。

use std::sync::OnceLock;

#[derive(Debug)]
pub struct Trace {
    which: &'static str,
    on: OnceLock<bool>,
}

impl Trace {
    pub const fn of(which: &'static str) -> Self {
        Self {
            which,
            on: OnceLock::new(),
        }
    }

    /// 環境変数は一度だけ読む（フレームごとに呼ばれる場所がある）。
    pub fn on(&self) -> bool {
        *self.on.get_or_init(|| {
            asked(
                std::env::var_os("ZW_TRACE"),
                std::env::var_os("ZEROWAVE_TRACE"),
                self.which,
            )
        })
    }
}

fn asked(short: Option<std::ffi::OsString>, long: Option<std::ffi::OsString>, which: &str) -> bool {
    let Some(said) = short.or(long) else {
        return false;
    };
    let said = said.to_string_lossy().to_ascii_lowercase();
    said == "1" || said == "all" || said.split(',').any(|name| name.trim() == which)
}

pub fn since_start() -> u128 {
    static FROM: OnceLock<std::time::Instant> = OnceLock::new();
    FROM.get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn either_name_of_the_variable_is_read() {
        assert!(asked(Some("d3d12".into()), None, "d3d12"));
        assert!(asked(None, Some("d3d12".into()), "d3d12"));
    }

    #[test]
    fn a_name_that_was_not_asked_for_stays_quiet() {
        let said = || Some("v4l2, nvdec".into());
        assert!(asked(None, said(), "v4l2"));
        assert!(asked(None, said(), "nvdec"), "空白は無視する");
        assert!(!asked(None, said(), "d3d12"));
        assert!(asked(None, Some("1".into()), "d3d12"), "1 は全部");
        assert!(!asked(None, None, "v4l2"), "指定が無ければ出さない");
    }
}
