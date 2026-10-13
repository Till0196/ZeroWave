// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! coaxd を必要なときに起動し、設定ウィンドウからの追加・更新を受け持つ。探す順は `COAXD`、インストール先、`PATH`。
//! インストール先は Windows では deck と同じフォルダー、mac・Linux では設定フォルダー（バンドルの中や /usr/bin は書けない）。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub fn folder() -> Option<PathBuf> {
    if cfg!(any(target_os = "macos", target_os = "linux")) {
        return crate::state::file("coaxd")?
            .parent()
            .map(std::path::Path::to_path_buf);
    }
    Some(std::env::current_exe().ok()?.parent()?.to_path_buf())
}

pub fn file() -> Option<PathBuf> {
    Some(folder()?.join(file_name()))
}

pub fn install(launch: bool) {
    launch_or_not(launch);
    if let Some(folder) = folder() {
        std::thread::Builder::new()
            .name(String::from("coaxd tidy"))
            .spawn(move || tidy(&folder))
            .ok();
    }
}

pub fn launch_or_not(launch: bool) {
    zwcore::input::source::coaxd::launch_with(launch.then(|| {
        zwcore::input::source::coaxd::Launcher {
            named: None,
            beside: folder(),
        }
    }));
}

pub const fn file_name() -> &'static str {
    if cfg!(windows) { "coaxd.exe" } else { "coaxd" }
}

const STAGED: &str = if cfg!(windows) {
    ".coaxd.new.exe"
} else {
    ".coaxd.new"
};

/// 更新で退避した物（Windows は動いている exe を消せないので名前だけ替えておく）。
const OLD: &str = "coaxd.old";

/// インストール先の coaxd と動いている coaxd の状態。coaxd を実行して問い合わせるのでウィンドウのスレッドでは作らない。
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub present: bool,
    pub version: String,
    pub arch: String,
    pub put_at: String,
    pub running: Option<zwcore::input::source::coaxd::Running>,
    pub ours: Option<bool>,
}

impl Seen {
    pub fn running_file(&self) -> Option<&Path> {
        self.running.as_ref()?.executable.as_deref()
    }
}

pub fn look(endpoint: &str) -> Seen {
    let file = file();
    let present = file.as_deref().is_some_and(Path::is_file);
    let mut seen = Seen {
        present,
        ..Seen::default()
    };
    if let Some(file) = file.as_deref().filter(|_| present) {
        if let Ok(binary) = inspect(file) {
            seen.arch = shown_arch(binary.arch).to_owned();
        }
        seen.version = run_version(file).unwrap_or_default();
        seen.put_at = std::fs::metadata(file)
            .and_then(|meta| meta.modified())
            .map(japan_time)
            .unwrap_or_default();
    }
    seen.running = zwcore::input::source::coaxd::running(endpoint);
    seen.ours = seen
        .running
        .as_ref()
        .and_then(|running| running.executable.as_deref())
        .map(|running| file.as_deref().is_some_and(|file| same_file(running, file)));
    seen
}

fn same_file(one: &Path, other: &Path) -> bool {
    match (one.canonicalize(), other.canonicalize()) {
        (Ok(one), Ok(other)) => one == other,
        _ => one == other,
    }
}

fn japan_time(at: SystemTime) -> String {
    let Ok(since) = at.duration_since(SystemTime::UNIX_EPOCH) else {
        return String::new();
    };
    let seconds = i64::try_from(since.as_secs()).unwrap_or_default() + 9 * 3600;
    let (_, month, day, hour, minute, _) = zwcore::guide::civil(seconds);
    format!("{month}/{day} {hour}:{minute:02}")
}

pub fn japan_time_of(rfc3339: &str) -> String {
    let Ok(parsed) = rfc3339.parse::<toml::value::Datetime>() else {
        return String::new();
    };
    let (Some(date), Some(time)) = (parsed.date, parsed.time) else {
        return String::new();
    };
    let offset = match parsed.offset {
        Some(toml::value::Offset::Custom { minutes }) => i64::from(minutes) * 60,
        _ => 0,
    };
    let days = days_from_civil(
        i64::from(date.year),
        u32::from(date.month),
        u32::from(date.day),
    );
    let seconds = days * 86_400
        + i64::from(time.hour) * 3600
        + i64::from(time.minute) * 60
        + i64::from(time.second.unwrap_or(0))
        - offset;
    let Ok(seconds) = u64::try_from(seconds) else {
        return String::new();
    };
    japan_time(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binary {
    pub os: &'static str,
    pub arch: &'static str,
}

fn this_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    }
}

pub fn shown_arch(arch: &str) -> &str {
    match arch {
        "amd64" => "x86_64",
        other => other,
    }
}

pub fn inspect(path: &Path) -> Result<Binary, String> {
    use std::io::Read;
    let mut head = vec![0_u8; 4096];
    let read = std::fs::File::open(path)
        .and_then(|mut file| file.read(&mut head))
        .map_err(|why| format!("ファイルを読めません（{why}）"))?;
    head.truncate(read);
    binary_of(&head).ok_or_else(|| String::from("実行できるファイルではありません"))
}

fn binary_of(head: &[u8]) -> Option<Binary> {
    let u16_le = |at: usize| Some(u16::from_le_bytes(head.get(at..at + 2)?.try_into().ok()?));
    let u32_le = |at: usize| Some(u32::from_le_bytes(head.get(at..at + 4)?.try_into().ok()?));
    let u32_be = |at: usize| Some(u32::from_be_bytes(head.get(at..at + 4)?.try_into().ok()?));
    let mach = |cpu: u32| match cpu {
        0x0100_000C => Some("arm64"),
        0x0100_0007 => Some("amd64"),
        _ => None,
    };
    match head.get(..4)? {
        [0xCF, 0xFA, 0xED, 0xFE] => Some(Binary {
            os: "macos",
            arch: mach(u32_le(4)?)?,
        }),
        // ユニバーサル（fat）の Mach-O。Java の class も同じマジックなので、数が小さいときだけ。
        [0xCA, 0xFE, 0xBA, 0xBE] => {
            let count = u32_be(4)? as usize;
            if count == 0 || count > 8 {
                return None;
            }
            let arches: Vec<&'static str> = (0..count)
                .filter_map(|index| mach(u32_be(8 + index * 20)?))
                .collect();
            let arch = arches
                .iter()
                .copied()
                .find(|arch| *arch == this_arch())
                .or_else(|| arches.first().copied())?;
            Some(Binary { os: "macos", arch })
        }
        [b'M', b'Z', ..] => {
            let pe = u32_le(0x3C)? as usize;
            if head.get(pe..pe + 4)? != b"PE\0\0" {
                return None;
            }
            let arch = match u16_le(pe + 4)? {
                0x8664 => "amd64",
                0xAA64 => "arm64",
                0x014C => "386",
                _ => return None,
            };
            Some(Binary {
                os: "windows",
                arch,
            })
        }
        [0x7F, b'E', b'L', b'F'] => {
            let arch = match u16_le(18)? {
                0x3E => "amd64",
                0xB7 => "arm64",
                0x03 => "386",
                0x28 => "arm",
                _ => return None,
            };
            Some(Binary { os: "linux", arch })
        }
        _ => None,
    }
}

fn runs_here(binary: Binary) -> Result<(), String> {
    let os = std::env::consts::OS;
    let here = if os == "macos" {
        "この Mac"
    } else {
        "この PC"
    };
    if binary.os != os {
        let made_for = match binary.os {
            "macos" => "macOS",
            "windows" => "Windows",
            _ => "Linux",
        };
        return Err(format!("{here} では動作しません（{made_for} 用）"));
    }
    if binary.arch != this_arch() {
        return Err(format!(
            "{here} では動作しません（{} 用）",
            shown_arch(binary.arch)
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub path: PathBuf,
    pub picked: String,
    pub version: String,
    pub arch: String,
}

pub fn stage(from: &Path) -> Result<Staged, String> {
    let binary = inspect(from)?;
    runs_here(binary)?;
    allowed(from)?;
    let folder = folder().ok_or_else(|| String::from("インストール先が分かりません"))?;
    std::fs::create_dir_all(&folder).map_err(|why| format!("ファイルが書き込めません（{why}）"))?;
    let staged = folder.join(STAGED);
    let _ = std::fs::remove_file(&staged);
    std::fs::copy(from, &staged).map_err(|why| format!("ファイルが書き込めません（{why}）"))?;
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&staged) {
        let _ = file.set_modified(SystemTime::now());
    }
    let ready = prepare(&staged).and_then(|()| {
        said_version(&staged).ok_or_else(|| {
            String::from("起動できませんでした（coaxd ではないか、ファイルが壊れています）")
        })
    });
    match ready {
        Ok(version) => Ok(Staged {
            path: staged,
            picked: from
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            version,
            arch: shown_arch(binary.arch).to_owned(),
        }),
        Err(why) => {
            let _ = std::fs::remove_file(&staged);
            Err(why)
        }
    }
}

/// mac で quarantine 属性の付いたファイルは、属性を外す前に利用者に確かめる。
#[cfg(target_os = "macos")]
fn allowed(from: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let Ok(named) = std::ffi::CString::new(from.as_os_str().as_bytes()) else {
        return Ok(());
    };
    // SAFETY: 名前は NUL 終端で呼び出しの間生きている。サイズだけを問い合わせる（バッファは渡さない）。
    let size = unsafe {
        libc::getxattr(
            named.as_ptr(),
            c"com.apple.quarantine".as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if size < 0 {
        return Ok(());
    }
    let name = from
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = format!(
        "「{name}」はインターネットからダウンロードされたファイルです。\n\
         開発元を確認できません。入手先が信頼できる場合だけ、OK を押して追加してください。\n\n{}",
        from.display()
    );
    if crate::window::ask::ok_cancel("coaxd の追加", &text) {
        Ok(())
    } else {
        Err(String::from("追加を取りやめました"))
    }
}

#[cfg(not(target_os = "macos"))]
fn allowed(_from: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn prepare(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|why| format!("ファイルが書き込めません（{why}）"))?;
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(named) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            unsafe {
                libc::removexattr(named.as_ptr(), c"com.apple.quarantine".as_ptr(), 0);
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// `coaxd -version` の出力からバージョン。mac で署名の無い物は強制終了されるので ad-hoc 署名してもう一度。
fn said_version(path: &Path) -> Option<String> {
    match run_version(path) {
        Ok(said) => Some(said),
        #[cfg(target_os = "macos")]
        Err(Some(9)) => {
            let signed = std::process::Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if signed { run_version(path).ok() } else { None }
        }
        Err(_) => None,
    }
}

fn run_version(path: &Path) -> Result<String, Option<i32>> {
    use std::io::Read;
    let mut command = std::process::Command::new(path);
    command
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|_| None)?;
    // 出力は別のスレッドで読む（子プロセスがパイプのバッファより多く書くと止まる）。
    let reader = child.stdout.take().map(|out| {
        std::thread::spawn(move || {
            let mut out = out;
            let mut said = Vec::new();
            let _ = (&mut out).take(4096).read_to_end(&mut said);
            let _ = std::io::copy(&mut out, &mut std::io::sink());
            String::from_utf8_lossy(&said).into_owned()
        })
    });
    let began = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if began.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(None);
            }
        }
    };
    if !status.success() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            return Err(status.signal());
        }
        #[cfg(not(unix))]
        return Err(None);
    }
    let said = reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    version_of_line(said.lines().next().unwrap_or_default()).ok_or(None)
}

/// `coaxd 0.1.0 (abc1234) arm64` から `0.1.0（abc1234）`。先頭が coaxd でなければ `None`。
fn version_of_line(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("coaxd")?;
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    if words
        .last()
        .is_some_and(|word| ["amd64", "arm64", "386", "arm"].contains(word))
    {
        words.pop();
    }
    let joined = words.join(" ");
    Some(match joined.split_once(" (") {
        Some((version, commit)) => format!("{version}（{}）", commit.trim_end_matches(')')),
        None => joined,
    })
}

/// 確認したコピーを入れ、前の物は退避してその名前を返す。実行中の物は前の中身のまま動き続ける。
pub fn swap(staged: &Staged) -> Result<Option<PathBuf>, String> {
    let into = file().ok_or_else(|| String::from("インストール先が分かりません"))?;
    let failed = |why: std::io::Error| format!("ファイルが書き込めません（{why}）");
    let mut old = None;
    if into.exists() {
        let aside = aside_name(&into);
        // 上書きコピーしない：mac はカーネルが前の署名をキャッシュしていて、起動した途端に強制終了する。
        std::fs::rename(&into, &aside).map_err(failed)?;
        old = Some(aside);
    }
    if let Err(why) = std::fs::rename(&staged.path, &into) {
        if let Some(old) = &old {
            let _ = std::fs::rename(old, &into);
        }
        return Err(failed(why));
    }
    Ok(old)
}

pub fn restore(staged: &Staged, old: Option<&Path>) {
    let Some(into) = file() else {
        return;
    };
    if std::fs::rename(&into, &staged.path).is_ok()
        && let Some(old) = old
    {
        let _ = std::fs::rename(old, &into);
    }
}

pub fn discard(staged: &Staged) {
    let _ = std::fs::remove_file(&staged.path);
}

fn aside_name(into: &Path) -> PathBuf {
    let folder = into.parent().unwrap_or(Path::new("."));
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let first = folder.join(format!("{OLD}{extension}"));
    if !first.exists() || std::fs::remove_file(&first).is_ok() {
        return first;
    }
    (2..100)
        .map(|number| folder.join(format!("{OLD}-{number}{extension}")))
        .find(|candidate| !candidate.exists() || std::fs::remove_file(candidate).is_ok())
        .unwrap_or(first)
}

fn is_aside(name: &str) -> bool {
    let extension = if cfg!(windows) { ".exe" } else { "" };
    let Some(rest) = name
        .strip_prefix(OLD)
        .and_then(|rest| rest.strip_suffix(extension))
    else {
        return false;
    };
    rest.is_empty()
        || rest
            .strip_prefix('-')
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

/// 退避した物と残ったコピーを消す。確認用のコピーは確認待ちの間も要るので起動の時にだけ消す。
fn tidy(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_aside(&name) || name == STAGED {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_always_a_folder() {
        assert!(folder().is_some());
    }

    #[test]
    fn the_head_of_a_file_says_what_it_runs_on() {
        let mut mach = vec![0xCF, 0xFA, 0xED, 0xFE, 0x0C, 0x00, 0x00, 0x01];
        mach.resize(64, 0);
        assert_eq!(
            binary_of(&mach),
            Some(Binary {
                os: "macos",
                arch: "arm64"
            })
        );
        let mut pe = vec![0_u8; 0x100];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C] = 0x80;
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        assert_eq!(
            binary_of(&pe),
            Some(Binary {
                os: "windows",
                arch: "amd64"
            })
        );
        let mut elf = vec![0x7F, b'E', b'L', b'F'];
        elf.resize(20, 0);
        elf[18] = 0xB7;
        assert_eq!(
            binary_of(&elf),
            Some(Binary {
                os: "linux",
                arch: "arm64"
            })
        );
        assert_eq!(binary_of(b"#!/bin/sh\n"), None);
    }

    #[test]
    fn a_universal_binary_answers_for_this_machine() {
        let mut fat = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 2];
        for cpu in [0x0100_0007_u32, 0x0100_000C] {
            fat.extend_from_slice(&cpu.to_be_bytes());
            fat.extend_from_slice(&[0; 16]);
        }
        let binary = binary_of(&fat).expect("Mach-O");
        if ["amd64", "arm64"].contains(&this_arch()) {
            assert_eq!(binary.arch, this_arch());
        }
    }

    #[test]
    fn the_version_line_becomes_the_shown_version() {
        assert_eq!(
            version_of_line("coaxd 0.1.0 (abc1234) arm64").as_deref(),
            Some("0.1.0（abc1234）")
        );
        assert_eq!(
            version_of_line("coaxd 0.1.0 (abc1234)").as_deref(),
            Some("0.1.0（abc1234）")
        );
        assert_eq!(version_of_line("coaxd 0.1.0").as_deref(), Some("0.1.0"));
        assert_eq!(version_of_line("something else"), None);
    }

    #[test]
    #[ignore = "本物の coaxd が要る"]
    fn a_real_coaxd_goes_in_and_is_replaced() {
        let sample = PathBuf::from(std::env::var("COAXD_SAMPLE").expect("COAXD_SAMPLE"));
        let into = file().expect("インストール先");
        assert!(!into.exists(), "空のインストール先で実行する");
        let staged = stage(&sample).expect("コピーできる");
        println!("{} {} {}", staged.picked, staged.version, staged.arch);
        assert!(!staged.version.is_empty());
        assert_eq!(swap(&staged).expect("入れられる"), None);
        assert!(into.is_file());
        let again = stage(&sample).expect("もう一度コピーできる");
        let old = swap(&again)
            .expect("入れ替えられる")
            .expect("前の物が退避される");
        assert!(into.is_file() && old.is_file());
        restore(&again, Some(&old));
        assert!(into.is_file() && again.path.is_file() && !old.exists());
        discard(&again);
        let seen = look("");
        println!("{seen:?}");
        assert!(seen.present && !seen.version.is_empty());
        let _ = std::fs::remove_file(&into);
    }

    #[test]
    fn only_set_aside_names_are_tidied() {
        let extension = if cfg!(windows) { ".exe" } else { "" };
        assert!(is_aside(&format!("coaxd.old{extension}")));
        assert!(is_aside(&format!("coaxd.old-12{extension}")));
        assert!(!is_aside(&format!("coaxd.old.backup{extension}")));
        assert!(!is_aside(&format!("coaxd.old-{extension}")));
        assert!(!is_aside("coaxd"));
    }

    #[test]
    fn the_start_time_is_shown_in_japan_time() {
        assert_eq!(japan_time_of("2026-08-19T09:41:00Z"), "8/19 18:41");
        assert_eq!(japan_time_of(""), "");
    }
}
