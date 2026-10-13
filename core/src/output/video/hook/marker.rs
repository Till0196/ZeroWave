// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 測っている間に置くマーカー。GPU が失われて落ちたら残り、次の起動で見つける。

use super::*;

/// プロセスごとの名前でロックを持ち、ほかの deck が測っているマーカーを落ちた跡と取り違えない。
#[derive(Debug)]
pub(super) struct Marker {
    path: PathBuf,
    _held: std::fs::File,
}

impl Marker {
    pub(super) fn place(base: &Path, name: &str) -> Option<Self> {
        use std::io::Write;
        let mut file_name = base.file_name()?.to_os_string();
        file_name.push(format!(".{}", std::process::id()));
        let path = base.with_file_name(file_name);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut file = std::fs::File::create(&path).ok()?;
        file.write_all(name.as_bytes()).ok()?;
        file.try_lock().ok()?;
        Some(Self { path, _held: file })
    }

    pub(super) fn clear(self) {
        let Self { path, _held } = self;
        drop(_held);
        let _ = std::fs::remove_file(path);
    }

    /// 落ちたプロセスが残したマーカー（ロックを誰も持っていない物）。
    pub(super) fn left_behind(base: &Path) -> Option<PathBuf> {
        if base.exists() {
            return Some(base.to_path_buf());
        }
        let prefix = format!("{}.", base.file_name()?.to_string_lossy());
        std::fs::read_dir(base.parent()?)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            })
            .find(|path| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .is_ok_and(|file| file.try_lock().is_ok())
            })
    }
}

pub(super) fn marker_of(plugin: &Plugin) -> PathBuf {
    let place = plugin
        .path()
        .parent()
        .map(zwsdk::host::cache_dir)
        .unwrap_or_else(std::env::temp_dir);
    let stem = plugin.path().file_stem().map_or_else(
        || plugin.name().to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    place.join(format!("{stem}.gpu-suspect"))
}
