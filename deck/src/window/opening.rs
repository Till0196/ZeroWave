// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 録画ファイルを選ぶ OS のダイアログ。結果は別のスレッドが待って再生スレッドへ [`Act::Open`] で届ける。

use std::path::{Path, PathBuf};

use crate::threads::link::{Act, ToStage};

pub const EXTENSIONS: &[&str] = &["ts", "m2ts", "mmts", "tlv"];

/// 再生できるか。拡張子ではなく中身で（TS か MMT/TLV か）。
pub fn playable(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|text| zwcore::filters::Container::of_file(text).is_some())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn pick_plugin() -> Option<PathBuf> {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("プラグインを開く")
        .add_filter("プラグイン", &["wasm"]);
    let picked = pollster::block_on(dialog.pick_file())?;
    Some(picked.path().to_path_buf())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pick_plugin() -> Option<PathBuf> {
    let picked = std::process::Command::new("zenity")
        .args([
            "--file-selection",
            "--title=プラグインを開く",
            "--file-filter=プラグイン | *.wasm",
        ])
        .output();
    match picked {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            Some(PathBuf::from(text.trim_end_matches('\n')))
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!("プラグイン選択ダイアログを表示できない（zenity が無い）: {error}");
            None
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn pick_file(title: &str, filters: &[(String, Vec<String>)]) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
    for (name, extensions) in filters {
        dialog = dialog.add_filter(name, extensions);
    }
    let picked = pollster::block_on(dialog.pick_file())?;
    Some(picked.path().to_path_buf())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pick_file(title: &str, filters: &[(String, Vec<String>)]) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg(format!("--title={title}"));
    for (name, extensions) in filters {
        let globs: Vec<String> = extensions.iter().map(|e| format!("*.{e}")).collect();
        command.arg(format!("--file-filter={name} | {}", globs.join(" ")));
    }
    match command.output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            Some(PathBuf::from(text.trim_end_matches('\n')))
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!("ファイル選択ダイアログを表示できない（zenity が無い）: {error}");
            None
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn pick_program(title: &str) -> Option<PathBuf> {
    let dialog = rfd::AsyncFileDialog::new().set_title(title);
    let picked = pollster::block_on(dialog.pick_file())?;
    Some(picked.path().to_path_buf())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pick_program(title: &str) -> Option<PathBuf> {
    let picked = std::process::Command::new("zenity")
        .arg("--file-selection")
        .arg(format!("--title={title}"))
        .output();
    match picked {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            Some(PathBuf::from(text.trim_end_matches('\n')))
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!("プログラム選択ダイアログを表示できない（zenity が無い）: {error}");
            None
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn pick_folder(title: &str, start: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
    if let Some(start) = start.filter(|start| start.is_dir()) {
        dialog = dialog.set_directory(start);
    }
    let picked = pollster::block_on(dialog.pick_folder())?;
    Some(picked.path().to_path_buf())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pick_folder(title: &str, start: Option<PathBuf>) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--directory")
        .arg(format!("--title={title}"));
    if let Some(start) = start.filter(|start| start.is_dir()) {
        command.arg(format!("--filename={}/", start.display()));
    }
    match command.output() {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            Some(PathBuf::from(text.trim_end_matches('\n')))
        }
        Ok(_) => None,
        Err(error) => {
            tracing::warn!("フォルダー選択ダイアログを表示できない（zenity が無い）: {error}");
            None
        }
    }
}

/// ダイアログを出して再生スレッドへ [`Act::Open`] を送る。待たない（同期のダイアログだと閉じるまで OSD が止まる）。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn ask(to_stage: std::sync::mpsc::Sender<ToStage>) {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("録画を開く")
        .add_filter("録画", EXTENSIONS)
        .add_filter("すべてのファイル", &["*"]);
    let spawned = std::thread::Builder::new()
        .name(String::from("open"))
        .spawn(move || {
            let picked = pollster::block_on(dialog.pick_file());
            if let Some(picked) = picked {
                let path = picked.path().to_path_buf();
                if playable(&path) {
                    let _ = to_stage.send(ToStage::Act(Act::Open(path)));
                } else {
                    tracing::info!("{} は TS でも MMT/TLV でもない", path.display());
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("録画ファイル選択ダイアログのスレッドを起動できない: {error}");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn ask(to_stage: std::sync::mpsc::Sender<ToStage>) {
    let filter = EXTENSIONS
        .iter()
        .map(|extension| format!("*.{extension}"))
        .collect::<Vec<_>>()
        .join(" ");
    let spawned = std::thread::Builder::new()
        .name(String::from("open"))
        .spawn(move || {
            let picked = std::process::Command::new("zenity")
                .args([
                    "--file-selection",
                    "--title=録画を開く",
                    &format!("--file-filter=録画 | {filter}"),
                    "--file-filter=すべてのファイル | *",
                ])
                .output();
            match picked {
                Ok(output) if output.status.success() => {
                    let text = String::from_utf8_lossy(&output.stdout);
                    let path = PathBuf::from(text.trim_end_matches('\n'));
                    if playable(&path) {
                        let _ = to_stage.send(ToStage::Act(Act::Open(path)));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        "録画ファイル選択ダイアログを表示できない（zenity が無い）: {error}"
                    );
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("録画ファイル選択ダイアログのスレッドを起動できない: {error}");
    }
}

pub fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(path);
    match command.spawn() {
        // 回収はする（捨てると Unix ではゾンビが残る）。
        Ok(mut child) => {
            let _ = std::thread::Builder::new()
                .name(String::from("reveal"))
                .spawn(move || child.wait());
        }
        Err(why) => tracing::warn!("{} を開けない: {why}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recordings_are_playable_by_content_or_name() {
        assert!(playable(Path::new("a/録画.ts")));
        assert!(playable(Path::new("a.MMTS")));
        assert!(playable(Path::new("bs4k.tlv")));
        assert!(!playable(Path::new("readme.txt")));
        assert!(!playable(Path::new("noext")));
        let folder = std::env::temp_dir().join(format!("zw-open-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let named = folder.join("録画.txt");
        let mut ts = vec![0u8; 188 * 5];
        for nth in 0..5 {
            ts[nth * 188] = 0x47;
        }
        std::fs::write(&named, &ts).unwrap();
        assert!(playable(&named));
        std::fs::write(&named, b"not a stream").unwrap();
        assert!(!playable(&named));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
