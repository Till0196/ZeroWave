// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Stamps the build with the commit it came from.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
    // 環境変数が最優先（`.git` を含めずにコピーした先では `git describe` が失敗する）。
    println!("cargo:rerun-if-env-changed=ZW_BUILD");
    let stamp = std::env::var("ZW_BUILD")
        .ok()
        .map(|given| given.trim().to_string())
        .filter(|given| !given.is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["describe", "--always", "--dirty"])
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|found| !found.is_empty())
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=ZW_BUILD={stamp}");
    // 「ZeroWave について」に出すビルド日（1970 年からの秒）。
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rustc-env=ZW_BUILT_AT={}", built());
    println!("cargo:rerun-if-changed=Cargo.lock");
    let wasmtime = std::fs::read_to_string("Cargo.lock")
        .ok()
        .and_then(|lock| locked(&lock, "wasmtime"))
        .unwrap_or_default();
    println!("cargo:rustc-env=ZW_WASMTIME={wasmtime}");
    windows_icon();
}

fn built() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|given| given.trim().parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs() as i64)
        })
}

fn locked(lock: &str, name: &str) -> Option<String> {
    let wanted = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == wanted {
            let version = lines.next()?.trim();
            return version
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_owned);
        }
    }
    None
}

/// Windows の exe にアイコンを埋め込む（.ico を .res にしてリンカーに渡す。rc.exe は要らない）。
fn windows_icon() {
    println!("cargo:rerun-if-changed=icon/zerowave.ico");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let abi = std::env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    // MinGW の ld は .res を読まない（windres で COFF にする必要がある）。
    if os != "windows" || !(env == "msvc" || abi == "llvm") {
        return;
    }
    let Ok(ico) = std::fs::read("icon/zerowave.ico") else {
        println!("cargo:warning=icon/zerowave.ico が読めないので、exe にアイコンを付けない");
        return;
    };
    let Some(res) = resources(&ico) else {
        println!("cargo:warning=icon/zerowave.ico の形式が違うので、exe にアイコンを付けない");
        return;
    };
    let out =
        std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("deck.res");
    if std::fs::write(&out, res).is_err() {
        println!("cargo:warning={} に書けない", out.display());
        return;
    }
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

fn resources(ico: &[u8]) -> Option<Vec<u8>> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    let u16_at = |at: usize| Some(u16::from_le_bytes(ico.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(ico.get(at..at + 4)?.try_into().ok()?));
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return None;
    }
    let count = u16_at(4)?;
    // 頭に空のリソースを一つ置くのが .res の決まり。
    let mut res = entry(0, 0, &[], 0);
    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&count.to_le_bytes());
    for index in 0..count {
        let at = 6 + 16 * usize::from(index);
        let head = ico.get(at..at + 12)?;
        let size = u32_at(at + 8)? as usize;
        let offset = u32_at(at + 12)? as usize;
        let image = ico.get(offset..offset + size)?;
        let id = index + 1;
        res.extend(entry(RT_ICON, id, image, 0x1010));
        group.extend_from_slice(head);
        group.extend_from_slice(&id.to_le_bytes());
    }
    res.extend(entry(RT_GROUP_ICON, 1, &group, 0x1030));
    Some(res)
}

fn entry(kind: u16, name: u16, data: &[u8], flags: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + data.len() + 3);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&name.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}
