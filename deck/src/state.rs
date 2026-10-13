// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 設定・状態・プラグイン・coaxd の保存先。既定はバイナリの隣、`ZW_STATE_DIR`、OS の決まりの場所。

use std::path::PathBuf;

pub fn file(name: &str) -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("ZW_STATE_DIR") {
        return Some(PathBuf::from(base).join(name));
    }
    if let Some(beside) = beside_binary(name) {
        return Some(beside);
    }
    let base = if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|data| !data.is_empty()) {
        PathBuf::from(data)
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".local/share")
    };
    Some(base.join("zerowave").join(name))
}

fn beside_binary(name: &str) -> Option<PathBuf> {
    let binary = std::env::current_exe().ok()?;
    let folder = binary.parent()?;
    // バンドル（`.app`）の中には置かない。バンドルを差し替えると設定ごと消える。
    if cfg!(target_os = "macos")
        && folder.components().any(|part| {
            part.as_os_str()
                .to_str()
                .is_some_and(|name| name.ends_with(".app"))
        })
    {
        return None;
    }
    let path = folder.join(name);
    let writable = match std::fs::OpenOptions::new().append(true).open(&path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let probe = folder.join(".zerowave-write-probe");
            let ok = std::fs::File::create(&probe).is_ok();
            let _ = std::fs::remove_file(&probe);
            ok
        }
        Err(_) => false,
    };
    writable.then_some(path)
}

pub fn epg_cache() -> Option<PathBuf> {
    if std::env::var_os("ZW_EPG_CACHE").is_some_and(|v| v == "0") {
        return None;
    }
    file("epg.sqlite")
}

pub fn nvram() -> Option<PathBuf> {
    if std::env::var_os("ZW_NVRAM").is_some_and(|v| v == "0") {
        return None;
    }
    file("nvram.txt")
}

/// データ放送のプラグインに渡す設定（`ZW_MODEL_ID` などの環境変数で変えられる）。
pub fn plugin_cfg() -> std::collections::BTreeMap<String, String> {
    [
        ("receiver.model_id", "ZW_MODEL_ID"),
        ("receiver.maker_id", "ZW_MAKER_ID"),
        ("receiver.maker_oui", "ZW_MAKER_OUI"),
        ("no_net", "ZW_NO_NET"),
        ("trace", "ZW_TRACE"),
    ]
    .into_iter()
    .filter_map(|(name, var)| Some((String::from(name), std::env::var(var).ok()?)))
    .collect()
}
