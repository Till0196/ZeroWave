// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ファイルを読む（`zw_files_v1`）。宣言の既定値はプラグインのフォルダーの下だけ。
//! `..` と外を指すリンクはたどらない。書き込み API は無い。

use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};

use wasmtime::{Caller, Linker};

use crate::abi;
use crate::host::{HostState, Io, Level, guest_text};

const READ_MOST: u64 = 64 << 20;

const LIST_MOST: usize = 4096;

impl Io {
    /// ユーザーの値はそのまま、既定値はプラグインのフォルダーの下だけ。
    pub fn folder(&self, key: &str) -> Option<PathBuf> {
        if !self.folders.contains(key) {
            return None;
        }
        let home = self.home.as_deref();
        if let Some(chosen) = self.chosen_value(key) {
            let chosen = chosen.trim();
            if !chosen.is_empty() {
                let path = PathBuf::from(chosen);
                return Some(if path.is_absolute() {
                    path
                } else {
                    home?.join(path)
                });
            }
        }
        let default = self.defaults.get(key)?;
        let relative = Path::new(default.trim());
        let inside = relative
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
        inside.then(|| home.map(|home| home.join(relative)))?
    }
}

fn under(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let target = root.join(relative).canonicalize().ok()?;
    target.starts_with(&root).then_some(target)
}

/// 一行に一つ、`f<TAB>大きさ<TAB>名前` か `d<TAB>0<TAB>名前`。名前の順。
fn list(root: &Path, relative: &str) -> Option<String> {
    let dir = under(root, relative)?;
    let mut lines: Vec<(String, String)> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(Result::ok)
        .take(LIST_MOST)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = under(root, &Path::new(relative).join(&name).to_string_lossy())?;
            let meta = std::fs::metadata(&path).ok()?;
            let line = if meta.is_dir() {
                format!("d\t0\t{name}")
            } else {
                format!("f\t{}\t{name}", meta.len())
            };
            Some((name, line))
        })
        .collect();
    lines.sort();
    Some(
        lines
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn read(root: &Path, relative: &str) -> Result<Vec<u8>, String> {
    let path = under(root, relative).ok_or_else(|| format!("{relative} は読めない"))?;
    let meta = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err(format!("{relative} はファイルではない"));
    }
    if meta.len() > READ_MOST {
        return Err(format!("{relative} は大きすぎる（{} バイト）", meta.len()));
    }
    std::fs::read(&path).map_err(|error| error.to_string())
}

/// 名前・大きさ・時刻（二階層まで）のフィンガープリント。変わればビルドし直す。
pub fn fingerprint(io: &Io) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for key in &io.folders {
        let Some(root) = io.folder(key) else {
            continue;
        };
        key.hash(&mut hasher);
        walk(&root, 2, &mut hasher);
    }
    hasher.finish()
}

fn walk(dir: &Path, depth: u32, hasher: &mut impl Hasher) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).take(LIST_MOST).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        // Windows の `DirEntry::metadata` は書き換えた直後でも前の値が返ることがあるので、ファイルから直接取得する。
        let Ok(meta) = std::fs::metadata(entry.path()) else {
            continue;
        };
        entry.file_name().hash(hasher);
        meta.len().hash(hasher);
        if let Ok(modified) = meta.modified() {
            modified.hash(hasher);
        }
        if meta.is_dir() && depth > 0 {
            walk(&entry.path(), depth - 1, hasher);
        }
    }
}

pub(crate) fn link(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    linker.func_wrap(
        abi::import::FILES,
        "list",
        |mut caller: Caller<'_, HostState>,
         key: u32,
         key_len: u32,
         dir: u32,
         dir_len: u32|
         -> u32 {
            let key = guest_text(&mut caller, key, key_len);
            let dir = guest_text(&mut caller, dir, dir_len);
            let state = caller.data_mut();
            let Some(root) = state.io.folder(&key) else {
                state.log(
                    Level::Warning,
                    &format!("zw_files_v1: キー {key} は kind = \"folder\" と宣言されていない"),
                );
                return 0;
            };
            match list(&root, &dir) {
                Some(text) => state.put_buf(text.into_bytes()),
                None => 0,
            }
        },
    )?;
    linker.func_wrap(
        abi::import::FILES,
        "read",
        |mut caller: Caller<'_, HostState>,
         key: u32,
         key_len: u32,
         path: u32,
         path_len: u32|
         -> u32 {
            let key = guest_text(&mut caller, key, key_len);
            let path = guest_text(&mut caller, path, path_len);
            let state = caller.data_mut();
            let Some(root) = state.io.folder(&key) else {
                state.log(
                    Level::Warning,
                    &format!("zw_files_v1: キー {key} は kind = \"folder\" と宣言されていない"),
                );
                return 0;
            };
            match read(&root, &path) {
                Ok(bytes) => state.put_buf(bytes),
                Err(why) => {
                    state.log(Level::Warning, &format!("zw_files_v1: {why}"));
                    0
                }
            }
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("zw-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("plugin/shaders/sub")).unwrap();
        std::fs::write(root.join("plugin/shaders/a.glsl"), b"aaa").unwrap();
        std::fs::write(root.join("plugin/shaders/sub/b.wgsl"), b"b").unwrap();
        std::fs::write(root.join("secret"), b"no").unwrap();
        root
    }

    fn io(root: &Path) -> Io {
        Io {
            folders: [String::from("shaders")].into(),
            home: Some(root.join("plugin")),
            defaults: [(String::from("shaders"), String::from("shaders"))].into(),
            ..Io::default()
        }
    }

    #[test]
    fn a_declared_folder_lists_and_reads_and_stays_inside() {
        let root = place("inside");
        let io = io(&root);
        let folder = io.folder("shaders").expect("宣言したキー");
        assert_eq!(folder, root.join("plugin/shaders"));
        assert_eq!(
            list(&folder, "").as_deref(),
            Some("f\t3\ta.glsl\nd\t0\tsub")
        );
        assert_eq!(read(&folder, "sub/b.wgsl").as_deref(), Ok(&b"b"[..]));
        assert!(read(&folder, "../../secret").is_err(), "外へは出ない");
        assert!(read(&folder, "/etc/passwd").is_err(), "絶対パスは読まない");
        assert_eq!(io.folder("other"), None, "宣言していないキー");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_the_user_may_point_outside_the_plugin() {
        let root = place("outside");
        let mut io = io(&root);
        io.defaults.insert("shaders".into(), "/".into());
        assert_eq!(io.folder("shaders"), None, "既定値の絶対パス");
        io.defaults.insert("shaders".into(), "../..".into());
        assert_eq!(io.folder("shaders"), None, "既定の ..");
        io.cfg
            .insert("shaders".into(), root.to_string_lossy().into_owned());
        assert_eq!(io.folder("shaders"), Some(root.clone()), "ユーザーの値");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_folder_is_not_followed() {
        let root = place("link");
        let folder = root.join("plugin/shaders");
        std::os::unix::fs::symlink(root.join("secret"), folder.join("leak")).unwrap();
        assert!(read(&folder, "leak").is_err());
        assert!(!list(&folder, "").unwrap().contains("leak"), "一覧に無い");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_fingerprint_follows_the_files() {
        let root = place("stamp");
        let io = io(&root);
        let before = fingerprint(&io);
        assert_eq!(before, fingerprint(&io));
        std::fs::write(root.join("plugin/shaders/sub/b.wgsl"), b"changed").unwrap();
        assert_ne!(before, fingerprint(&io));
        let _ = std::fs::remove_dir_all(&root);
    }
}
