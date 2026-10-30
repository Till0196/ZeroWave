// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! アダプターの一覧と選び方、子プロセスでの確認、デバイスロストの記録（DRED）。

use super::*;

/// デバイスロストの記録（DRED）を有効にする。デバイスを作る前に要るので起動の最初に呼ぶ。
pub fn enable_dred() {
    let mut settings: Option<ID3D12DeviceRemovedExtendedDataSettings> = None;
    // SAFETY: `settings` is a live local the call writes at most once.
    if unsafe { D3D12GetDebugInterface(&mut settings) }.is_ok()
        && let Some(settings) = settings
    {
        // SAFETY: the interface is alive for the length of both calls.
        unsafe {
            settings.SetAutoBreadcrumbsEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
            settings.SetPageFaultEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
        }
    }
}

static CHILD_PROBE: AtomicBool = AtomicBool::new(false);

/// 子プロセスにビデオデコードキューを試させる環境変数。値はアダプターの LUID（`上位:下位`）。
const PROBE_VARIABLE: &str = "ZW_PROBE_D3D12_VIDEO";

/// この実行ファイルを、ビデオデコードキューを試すだけの子プロセスとして起動してよいことを登録する。
pub fn enable_child_probe() {
    CHILD_PROBE.store(true, Ordering::Release);
}

/// 子プロセスとして起動されたならビデオデコードキューを試して終了コードを返す。ログもウィンドウも開く前に呼ぶ。
pub fn probe_child() -> Option<i32> {
    let value = std::env::var(PROBE_VARIABLE).ok()?;
    let (high, low) = value.split_once(':')?;
    let luid = windows::Win32::Foundation::LUID {
        HighPart: high.parse().ok()?,
        LowPart: low.parse().ok()?,
    };
    Some(i32::from(probe_on(luid).is_err()))
}

fn probe_on(luid: windows::Win32::Foundation::LUID) -> Result<(), Error> {
    let device = device_by_luid(luid)?;
    Device::video_queue(&device).map(|_| ())
}

/// ビデオデコードキューが作れるかを別のプロセスで先に確かめる（アダプターごとに一度）。
/// キューを作った瞬間にドライバーが壊れ、同じプロセスのデバイスが全部巻き添えになることがある。
pub fn probe_video(raw: &ID3D12Device) -> Result<(), Error> {
    type Answer = ((i32, u32), Result<(), String>);
    static PROBED: Mutex<Vec<Answer>> = Mutex::new(Vec::new());
    let luid = luid(raw);
    let key = (luid.HighPart, luid.LowPart);
    let Ok(mut probed) = PROBED.lock() else {
        return Err(Error::Unavailable(String::from(
            "確認結果のテーブルが壊れています",
        )));
    };
    if let Some((_, answer)) = probed.iter().find(|(seen, _)| *seen == key) {
        return answer.clone().map_err(Error::Unavailable);
    }
    let answer = match CHILD_PROBE
        .load(Ordering::Acquire)
        .then(|| probe_in_child(luid))
        .flatten()
    {
        Some(answer) => answer,
        None => probe_on(luid).map_err(|error| error.to_string()),
    };
    probed.push((key, answer.clone()));
    answer.map_err(Error::Unavailable)
}

#[derive(Debug, Clone)]
pub struct Adapter {
    pub name: String,
    pub luid: windows::Win32::Foundation::LUID,
    pub vendor: u32,
}

pub fn adapters() -> Vec<Adapter> {
    let Ok(factory) = factory::<IDXGIFactory1>() else {
        return Vec::new();
    };
    hardware_adapters(&factory, 16)
        .into_iter()
        .map(|(_, description)| {
            let end = description
                .Description
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(description.Description.len());
            Adapter {
                name: String::from_utf16_lossy(&description.Description[..end]),
                luid: description.AdapterLuid,
                vendor: description.VendorId,
            }
        })
        .collect()
}

/// レンダリングしている GPU の LUID。レンダリングがまだなら高性能のアダプター。
fn drawing_luid() -> Option<windows::Win32::Foundation::LUID> {
    if let Some(device) = adopted() {
        return Some(luid(&device));
    }
    let factory: IDXGIFactory6 = factory().ok()?;
    // SAFETY: 有効なファクトリーに値を渡すだけ。
    let adapter: IDXGIAdapter1 =
        unsafe { factory.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }
            .ok()?;
    Some(describe_adapter(&adapter)?.AdapterLuid)
}

pub fn draws_on(name: &str) -> bool {
    let Some(drawing) = drawing_luid() else {
        return false;
    };
    adapters()
        .iter()
        .any(|adapter| adapter.name == name && same_luid(adapter.luid, drawing))
}

pub(crate) fn same_luid(
    a: windows::Win32::Foundation::LUID,
    b: windows::Win32::Foundation::LUID,
) -> bool {
    a.HighPart == b.HighPart && a.LowPart == b.LowPart
}

fn probe_in_child(luid: windows::Win32::Foundation::LUID) -> Option<Result<(), String>> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().ok()?;
    let began = std::time::Instant::now();
    let mut child = std::process::Command::new(exe)
        .env(
            PROBE_VARIABLE,
            format!("{}:{}", luid.HighPart, luid.LowPart),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(NO_WINDOW)
        .spawn()
        .ok()?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if began.elapsed() < std::time::Duration::from_secs(10) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                break None;
            }
        }
    };
    let spent = began.elapsed().as_secs_f64() * 1000.0;
    Some(match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(format!(
            "子プロセスで試すとビデオデコードキューが作れません（終了 {:?}、{spent:.0} ms）",
            status.code()
        )),
        None => Err(String::from(
            "子プロセスでの確認が 10 秒でタイムアウトしました",
        )),
    })
}

pub fn name<T: Interface>(object: &T, text: &str) {
    let Ok(object) = object.cast::<ID3D12Object>() else {
        return;
    };
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the string is a live nul-terminated local; the call copies it.
    let _ = unsafe { object.SetName(windows::core::PCWSTR(wide.as_ptr())) };
}

/// デバイスロストの理由と、DRED が記録した内容を一度だけ書き出す。
pub fn report_removed() {
    static SAID: AtomicBool = AtomicBool::new(false);
    let Some(device) = adopted() else {
        return;
    };
    if SAID.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: the device is alive; the call only reads its state.
    let reason = unsafe { device.GetDeviceRemovedReason() };
    tracing::info!("D3D12: デバイスロストの理由 {reason:?}");
    let Ok(dred) = device.cast::<ID3D12DeviceRemovedExtendedData1>() else {
        tracing::warn!("D3D12: DRED を読めない");
        return;
    };
    let text = |wide: windows::core::PCWSTR| -> String {
        if wide.is_null() {
            String::from("（名前なし）")
        } else {
            // SAFETY: DRED's strings are nul-terminated and live as long as the device.
            unsafe { wide.to_string() }.unwrap_or_default()
        }
    };
    // SAFETY: the lists are DRED's, alive for the device's life, and only read.
    unsafe {
        if let Ok(output) = dred.GetAutoBreadcrumbsOutput1() {
            let mut node = output.pHeadAutoBreadcrumbNode;
            while let Some(here) = node.as_ref() {
                let done = here.pLastBreadcrumbValue.as_ref().copied().unwrap_or(0);
                let count = here.BreadcrumbCount;
                if done < count {
                    let history = std::slice::from_raw_parts(here.pCommandHistory, count as usize);
                    let from = done.saturating_sub(3) as usize;
                    let to = (done as usize + 2).min(history.len());
                    let ops: Vec<String> = (from..to)
                        .map(|at| {
                            let mark = if at == done as usize { "→" } else { "" };
                            format!("{mark}{}", breadcrumb(history[at]))
                        })
                        .collect();
                    tracing::info!(
                        "D3D12: キュー {} / コマンドリスト {}: {count} 個中 {done} 個実行済み  {}",
                        text(here.pCommandQueueDebugNameW),
                        text(here.pCommandListDebugNameW),
                        ops.join(" "),
                    );
                }
                node = here.pNext;
            }
        }
        if let Ok(fault) = dred.GetPageFaultAllocationOutput1() {
            if fault.PageFaultVA != 0 {
                tracing::info!("D3D12: ページフォルト 0x{:x}", fault.PageFaultVA);
            }
            for (what, head) in [
                ("その位置の割り当て", fault.pHeadExistingAllocationNode),
                (
                    "直前に解放した割り当て",
                    fault.pHeadRecentFreedAllocationNode,
                ),
            ] {
                let mut names = Vec::new();
                let mut node = head;
                while let Some(here) = node.as_ref() {
                    names.push(format!(
                        "{}（種類 {}）",
                        text(here.ObjectNameW),
                        here.AllocationType.0
                    ));
                    node = here.pNext;
                }
                if !names.is_empty() {
                    tracing::info!("D3D12: {what}: {}", names.join("、"));
                }
            }
        }
    }
}

fn breadcrumb(op: D3D12_AUTO_BREADCRUMB_OP) -> String {
    match op.0 {
        7 => "CopyBufferRegion".into(),
        8 => "CopyTextureRegion".into(),
        9 => "CopyResource".into(),
        3 => "Draw".into(),
        4 => "DrawIndexed".into(),
        6 => "Dispatch".into(),
        12 => "ClearRTV".into(),
        15 => "ResourceBarrier".into(),
        17 => "Present".into(),
        18 => "ResolveQueryData".into(),
        21 => "DecodeFrame".into(),
        27 => "DecodeFrame1".into(),
        29 => "DecodeFrame2".into(),
        45 => "Barrier".into(),
        other => format!("op{other}"),
    }
}
