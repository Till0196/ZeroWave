// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! この PC の情報（OS、CPU、GPU、ディスプレイ）。「ZeroWave について」に出す。

use zwcore::output::video::gpu::Gpu;

/// GPU やディスプレイの一つ。`mark` はレンダリングしている GPU と、ウィンドウのあるディスプレイに付ける。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub mark: Option<&'static str>,
    pub detail: String,
}

pub fn os() -> String {
    imp::os()
}

/// CPU の型番と種類。ソケットごとに一つ。
pub fn cpus() -> Vec<(String, String)> {
    let (models, arch) = imp::cpus();
    let bits = if arch.contains("64") { 64 } else { 32 };
    let kind = format!("{arch} · {bits} ビット");
    if models.is_empty() {
        return vec![(String::new(), kind)];
    }
    models
        .into_iter()
        .map(|model| (model, kind.clone()))
        .collect()
}

pub fn gpus(gpu: Option<&Gpu>) -> Vec<Unit> {
    let Some(gpu) = gpu else {
        return Vec::new();
    };
    let ours = gpu.adapter.get_info();
    gpu.adapters()
        .into_iter()
        .map(|info| {
            let drawing = info.backend == ours.backend
                && info.vendor == ours.vendor
                && info.device == ours.device
                && info.device_pci_bus_id == ours.device_pci_bus_id;
            let driver = [info.driver.trim(), info.driver_info.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let mut detail = backend(info.backend).to_owned();
            if !driver.is_empty() {
                detail.push_str(&format!(" · ドライバー {driver}"));
            }
            Unit {
                name: info.name,
                mark: drawing.then_some("レンダリング利用中"),
                detail,
            }
        })
        .collect()
}

pub fn rendering(gpu: Option<&Gpu>) -> String {
    gpu.map_or_else(
        || String::from("OpenGL ES 2.0"),
        |gpu| backend(gpu.adapter.get_info().backend).to_owned(),
    )
}

fn backend(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Dx12 => "Direct3D 12",
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Gl => "OpenGL ES",
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        wgpu::Backend::Noop => "なし",
    }
}

pub fn displays(window: &winit::window::Window) -> Vec<Unit> {
    // Wayland では同じディスプレイが二度並ぶことがある。名前と位置と大きさで同じと見る。
    let key = |monitor: &winit::monitor::MonitorHandle| {
        (monitor.name(), monitor.position(), monitor.size())
    };
    let here = window.current_monitor().as_ref().map(key);
    let mut seen = Vec::new();
    window
        .available_monitors()
        .filter(|monitor| {
            let one = key(monitor);
            let fresh = !seen.contains(&one);
            seen.push(one);
            fresh
        })
        .map(|monitor| {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let upright = if size.height > size.width {
                "（縦）"
            } else {
                ""
            };
            let pixels = if cfg!(target_os = "macos") {
                let wide = (f64::from(size.width) / scale).round();
                let tall = (f64::from(size.height) / scale).round();
                format!("{wide}×{tall}（{}×{}）{upright}", size.width, size.height)
            } else {
                format!("{}×{}{upright}", size.width, size.height)
            };
            let mut parts = vec![pixels];
            if let Some(millihertz) = monitor.refresh_rate_millihertz() {
                parts.push(format!("{} Hz", hertz(millihertz)));
            }
            let percent = (scale * 100.0).round();
            if cfg!(target_os = "macos") {
                parts.push(format!("拡大率 {percent}%"));
            } else {
                parts.push(format!(
                    "拡大率 {percent}%（{} dpi）",
                    (scale * 96.0).round()
                ));
            }
            let named = monitor.name().unwrap_or_default();
            Unit {
                name: imp::display_name(&named).unwrap_or(named),
                mark: (here.as_ref() == Some(&key(&monitor))).then_some("表示中"),
                detail: parts.join(" · "),
            }
        })
        .collect()
}

pub fn local_time(seconds: i64) -> String {
    if let Some((year, month, day, hour, minute)) = imp::local(seconds) {
        return format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}");
    }
    let (year, month, day) = civil(seconds.div_euclid(86_400));
    let of_day = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        of_day / 3600,
        of_day % 3600 / 60
    )
}

fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn hertz(millihertz: u32) -> String {
    if millihertz.is_multiple_of(1000) {
        format!("{}", millihertz / 1000)
    } else {
        let hertz = f64::from(millihertz) / 1000.0;
        let text = format!("{hertz:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::System::Registry::{RRF_RT_REG_DWORD, RRF_RT_REG_SZ};

    use crate::window::win32::registry_value as read;

    const CURRENT: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    fn text(key: &str, value: &str) -> Option<String> {
        let bytes = read(key, value, RRF_RT_REG_SZ)?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .take_while(|unit| *unit != 0)
            .collect();
        let text = String::from_utf16_lossy(&units).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    fn number(key: &str, value: &str) -> Option<u32> {
        let bytes = read(key, value, RRF_RT_REG_DWORD)?;
        Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
    }

    pub fn os() -> String {
        let build: Option<u32> = text(CURRENT, "CurrentBuildNumber").and_then(|b| b.parse().ok());
        let mut product = text(CURRENT, "ProductName").unwrap_or_else(|| String::from("Windows"));
        // Windows 11 でも ProductName は「Windows 10 …」のまま。11 はビルド 22000 から。
        if build.is_some_and(|build| build >= 22_000) {
            product = product.replacen("Windows 10", "Windows 11", 1);
        }
        let mut out = product;
        if let Some(version) = text(CURRENT, "DisplayVersion") {
            out.push(' ');
            out.push_str(&version);
        }
        if let Some(build) = build {
            out.push_str(&format!(" ビルド {build}"));
            if let Some(revision) = number(CURRENT, "UBR") {
                out.push_str(&format!(".{revision}"));
            }
        }
        out
    }

    pub fn cpus() -> (Vec<String>, String) {
        use windows::Win32::System::SystemInformation::{
            GetLogicalProcessorInformationEx, GetNativeSystemInfo, PROCESSOR_ARCHITECTURE_AMD64,
            PROCESSOR_ARCHITECTURE_ARM64, PROCESSOR_ARCHITECTURE_INTEL, RelationProcessorPackage,
            SYSTEM_INFO, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        };
        let mut info = SYSTEM_INFO::default();
        // SAFETY: 書く先はこちらの構造体。
        unsafe { GetNativeSystemInfo(&mut info) };
        // SAFETY: 共用体のどちらで読んでも 16 ビットの数。
        let arch = match unsafe { info.Anonymous.Anonymous.wProcessorArchitecture } {
            PROCESSOR_ARCHITECTURE_AMD64 => "x64",
            PROCESSOR_ARCHITECTURE_ARM64 => "arm64",
            PROCESSOR_ARCHITECTURE_INTEL => "x86",
            _ => std::env::consts::ARCH,
        }
        .to_owned();
        let mut length = 0u32;
        // SAFETY: サイズだけを問い合わせる（バッファが無いので失敗してサイズが返る）。
        let _ = unsafe {
            GetLogicalProcessorInformationEx(RelationProcessorPackage, None, &mut length)
        };
        let mut packages = 1usize;
        if length > 0 {
            let mut buffer = vec![0u8; length as usize];
            // SAFETY: `length` バイトのバッファ。
            let filled = unsafe {
                GetLogicalProcessorInformationEx(
                    RelationProcessorPackage,
                    Some(
                        buffer
                            .as_mut_ptr()
                            .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>(),
                    ),
                    &mut length,
                )
            };
            if filled.is_ok() {
                let mut at = 0usize;
                let mut counted = 0usize;
                while at + 8 <= length as usize {
                    let size =
                        u32::from_le_bytes(buffer[at + 4..at + 8].try_into().unwrap_or_default())
                            as usize;
                    if size == 0 {
                        break;
                    }
                    counted += 1;
                    at += size;
                }
                packages = counted.max(1);
            }
        }
        let model = text(
            r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "ProcessorNameString",
        );
        let models = model.map(|model| vec![model; packages]).unwrap_or_default();
        (models, arch)
    }

    pub fn display_name(gdi: &str) -> Option<String> {
        crate::draw::monitor::friendly_name(gdi)
    }

    pub fn local(seconds: i64) -> Option<(i64, i64, i64, i64, i64)> {
        use windows::Win32::Foundation::SYSTEMTIME;
        use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
        let (year, month, day) = super::civil(seconds.div_euclid(86_400));
        let of_day = seconds.rem_euclid(86_400);
        let utc = SYSTEMTIME {
            wYear: u16::try_from(year).ok()?,
            wMonth: month as u16,
            wDayOfWeek: 0,
            wDay: day as u16,
            wHour: (of_day / 3600) as u16,
            wMinute: (of_day % 3600 / 60) as u16,
            wSecond: (of_day % 60) as u16,
            wMilliseconds: 0,
        };
        let mut local = SYSTEMTIME::default();
        // SAFETY: 読む物も書く先もこちらの構造体。タイムゾーンは今の物（None）。
        unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }.ok()?;
        Some((
            i64::from(local.wYear),
            i64::from(local.wMonth),
            i64::from(local.wDay),
            i64::from(local.wHour),
            i64::from(local.wMinute),
        ))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    fn sysctl_text(name: &str) -> Option<String> {
        let name = std::ffi::CString::new(name).ok()?;
        let mut size = 0usize;
        // SAFETY: サイズだけを問い合わせる。
        if unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
            || size == 0
        {
            return None;
        }
        let mut buffer = vec![0u8; size];
        // SAFETY: `size` バイトのバッファ。
        if unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return None;
        }
        buffer.truncate(size);
        let text = String::from_utf8_lossy(&buffer)
            .trim_end_matches('\0')
            .trim()
            .to_owned();
        (!text.is_empty()).then_some(text)
    }

    fn sysctl_number(name: &str) -> Option<i32> {
        let name = std::ffi::CString::new(name).ok()?;
        let mut value = 0i32;
        let mut size = std::mem::size_of::<i32>();
        // SAFETY: 四バイトのバッファ。
        (unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut i32).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0)
            .then_some(value)
    }

    pub fn os() -> String {
        let version = sysctl_text("kern.osproductversion").unwrap_or_default();
        let major: u32 = version
            .split('.')
            .next()
            .and_then(|major| major.parse().ok())
            .unwrap_or(0);
        let named = match major {
            13 => " Ventura",
            14 => " Sonoma",
            15 => " Sequoia",
            26 => " Tahoe",
            _ => "",
        };
        let mut out = format!("macOS{named} {version}");
        if let Some(build) = sysctl_text("kern.osversion") {
            out.push_str(&format!(" ビルド {build}"));
        }
        out
    }

    pub fn cpus() -> (Vec<String>, String) {
        let translated = sysctl_number("sysctl.proc_translated") == Some(1);
        let arch = if translated || cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        let arch = if translated {
            format!("{arch}（Rosetta で動作）")
        } else {
            arch.to_owned()
        };
        let packages = sysctl_number("hw.packages").unwrap_or(1).max(1) as usize;
        let models = sysctl_text("machdep.cpu.brand_string")
            .map(|model| vec![model; packages])
            .unwrap_or_default();
        (models, arch)
    }

    pub fn display_name(_name: &str) -> Option<String> {
        None
    }

    pub(super) use super::unix_local as local;
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    pub fn os() -> String {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let pretty = release
            .lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map(|value| value.trim().trim_matches('"').to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| String::from("Linux"));
        let mut parts = vec![pretty];
        if let Ok(kernel) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
            parts.push(format!("Linux {}", kernel.trim()));
        }
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            parts.push(String::from("Wayland"));
        } else if std::env::var_os("DISPLAY").is_some() {
            parts.push(String::from("X11"));
        }
        parts.join(" · ")
    }

    pub fn cpus() -> (Vec<String>, String) {
        let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let mut models: Vec<(String, String)> = Vec::new();
        for block in info.split("\n\n") {
            let field = |key: &str| {
                block.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    (name.trim() == key).then(|| value.trim().to_owned())
                })
            };
            let Some(model) = field("model name").filter(|model| !model.is_empty()) else {
                continue;
            };
            let socket = field("physical id").unwrap_or_default();
            if !models.iter().any(|(known, _)| *known == socket) {
                models.push((socket, model));
            }
        }
        (
            models.into_iter().map(|(_, model)| model).collect(),
            machine(),
        )
    }

    fn machine() -> String {
        // SAFETY: 書く先はこちらの構造体。
        let mut names: libc::utsname = unsafe { std::mem::zeroed() };
        // SAFETY: 同上。
        if unsafe { libc::uname(&mut names) } != 0 {
            return std::env::consts::ARCH.to_owned();
        }
        // SAFETY: uname は NUL 終端の文字列を書く。
        unsafe { std::ffi::CStr::from_ptr(names.machine.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn display_name(_name: &str) -> Option<String> {
        None
    }

    pub(super) use super::unix_local as local;
}

#[cfg(unix)]
fn unix_local(seconds: i64) -> Option<(i64, i64, i64, i64, i64)> {
    let time = libc::time_t::try_from(seconds).ok()?;
    // SAFETY: 書く先はこちらの構造体。
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: 読む物も書く先もこちらの変数。
    if unsafe { libc::localtime_r(&time, &mut parts) }.is_null() {
        return None;
    }
    Some((
        i64::from(parts.tm_year) + 1900,
        i64::from(parts.tm_mon) + 1,
        i64::from(parts.tm_mday),
        i64::from(parts.tm_hour),
        i64::from(parts.tm_min),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_hertz_has_no_fraction() {
        assert_eq!(hertz(60_000), "60");
        assert_eq!(hertz(59_940), "59.94");
        assert_eq!(hertz(143_900), "143.9");
    }

    #[test]
    fn days_become_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_372), (2025, 10, 11));
    }

    #[test]
    fn a_time_is_written_to_the_minute() {
        let written = local_time(1_791_694_355);
        assert_eq!(written.len(), "2026-10-11 20:32".len());
    }

    #[test]
    fn a_cpu_says_how_many_bits() {
        assert!(cpus().iter().all(|(_, kind)| kind.contains("ビット")));
    }
}
