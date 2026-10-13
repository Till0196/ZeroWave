// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ウィンドウがあるモニターの色の扱い（HDR が有効か、SDR の白の輝度）。Windows 以外は値を返さない。
//! サーフェスが PQ を開けると返しても、SDR のモニターに表示すると白が潰れるのでモニターに問い合わせる。

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    /// HDR が有効か。偽ならサーフェスを HDR で開いてはいけない。
    pub hdr: bool,
    pub sdr_white: f32,
    pub peak: f32,
}

#[cfg(windows)]
fn active_paths() -> Option<Vec<windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO>> {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, GetDisplayConfigBufferSizes,
        QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
    };
    use windows::Win32::Foundation::ERROR_SUCCESS;

    let mut paths = 0u32;
    let mut modes = 0u32;
    // SAFETY: 二つの数を書く先はこちらの変数で、生きている。
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) }
        != ERROR_SUCCESS
    {
        return None;
    }
    let mut path_array = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
    let mut mode_array = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
    // SAFETY: 配列は問い合わせた数で確保してあり、関数は数を増やさない。
    if unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths,
            path_array.as_mut_ptr(),
            &mut modes,
            mode_array.as_mut_ptr(),
            None,
        )
    } != ERROR_SUCCESS
    {
        return None;
    }
    path_array.truncate(paths as usize);
    Some(path_array)
}

#[cfg(windows)]
fn named(path: &windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO, name: &str) -> bool {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    };
    let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: 先頭に大きさと種別を入れた構造体で、関数はその大きさまでしか書かない。
    if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0 {
        return false;
    }
    let end = source
        .viewGdiDeviceName
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(source.viewGdiDeviceName.len());
    source.viewGdiDeviceName[..end] == name.encode_utf16().collect::<Vec<u16>>()[..]
}

#[cfg(windows)]
pub fn friendly_name(name: &str) -> Option<String> {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    };
    let path = active_paths()?.into_iter().find(|path| named(path, name))?;
    let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: 先頭に大きさと種別を入れた構造体で、関数はその大きさまでしか書かない。
    if unsafe { DisplayConfigGetDeviceInfo(&mut target.header) } != 0 {
        return None;
    }
    let units = &target.monitorFriendlyDeviceName;
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(windows)]
pub fn of(name: &str) -> Option<Monitor> {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
        DisplayConfigGetDeviceInfo,
    };

    // 名前が合う最初の出力。
    let paths = active_paths()?;
    let path = paths.iter().find(|path| named(path, name))?;
    // ビットは wingdi.h の並びで、b0 が対応、b1 が有効。
    let mut advanced = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: 先頭に大きさと種別を入れた構造体で、関数はその大きさまでしか書かない。
    if unsafe { DisplayConfigGetDeviceInfo(&mut advanced.header) } != 0 {
        return None;
    }
    // SAFETY: 共用体を一つの u32 として読む（同じ四バイト）。
    let hdr = unsafe { advanced.Anonymous.value } & 0x2 != 0;
    let mut level = DISPLAYCONFIG_SDR_WHITE_LEVEL {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
            size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        SDRWhiteLevel: 0,
    };
    // SAFETY: 同上。
    if unsafe { DisplayConfigGetDeviceInfo(&mut level.header) } != 0 {
        return None;
    }
    Some(Monitor {
        hdr,
        sdr_white: level.SDRWhiteLevel as f32 / 1000.0 * 80.0,
        peak: peak_of(name).unwrap_or(zwcore::output::video::colour::HLG_PEAK),
    })
}

#[cfg(windows)]
fn peak_of(name: &str) -> Option<f32> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput6};
    use windows::core::Interface;

    // SAFETY: DXGI のファクトリを作って列挙するだけ。
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for a in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(a) else {
                break;
            };
            for o in 0.. {
                let Ok(output) = adapter.EnumOutputs(o) else {
                    break;
                };
                let Ok(output6) = output.cast::<IDXGIOutput6>() else {
                    continue;
                };
                let Ok(desc) = output6.GetDesc1() else {
                    continue;
                };
                let end = desc
                    .DeviceName
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(desc.DeviceName.len());
                let device = String::from_utf16_lossy(&desc.DeviceName[..end]);
                if device != name {
                    continue;
                }
                let peak = desc.MaxLuminance;
                return (peak.is_finite() && peak >= 100.0).then_some(peak);
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn of(_name: &str) -> Option<Monitor> {
    None
}
