// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! よく使う呼び出しの安全なラッパー（D3D11VA と NVDEC との連携でも使う）。

use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RANGE, D3D12_RESOURCE_DESC, D3D12CreateDevice,
    ID3D12Device, ID3D12DeviceChild, ID3D12Fence, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE,
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIAdapter1, IDXGIFactory1,
    IDXGIFactory4, IDXGIResource1,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{Interface, Param};

use super::failed;
use crate::decode::Error;

/// `GENERIC_ALL`。D3D12 の `CreateSharedHandle` に渡す権限。
const GENERIC_ALL: u32 = 0x1000_0000;

/// 自分で閉じる NT ハンドル（イベントと共有のハンドル）。
pub(crate) struct Owned(HANDLE);

impl Owned {
    /// 名前無し、自動リセット、非シグナル状態のイベント。
    pub fn event() -> Result<Self, Error> {
        // SAFETY: 名前も保護の属性も渡さない。
        unsafe { CreateEventW(None, false, false, None) }
            .map(Self)
            .map_err(|error| failed("CreateEventW", error))
    }

    /// D3D12 のオブジェクト（リソース・フェンス）を別のデバイスや API で開くためのハンドル。
    pub fn shared<P: Param<ID3D12DeviceChild>>(
        device: &ID3D12Device,
        object: P,
    ) -> Result<Self, Error> {
        // SAFETY: オブジェクトはこのデバイスのもの。属性も名前も渡さない。
        unsafe { device.CreateSharedHandle(object, None, GENERIC_ALL, None) }
            .map(Self)
            .map_err(|error| failed("CreateSharedHandle", error))
    }

    /// D3D11 のテクスチャを D3D12 で開くためのハンドル。
    pub fn shared_dxgi(resource: &IDXGIResource1) -> Result<Self, Error> {
        // SAFETY: 有効なリソース。属性も名前も渡さない。
        unsafe {
            resource.CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                None,
            )
        }
        .map(Self)
        .map_err(|error| failed("CreateSharedHandle", error))
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }

    /// フェンスが `value` に届くまで待つ。このイベントは自動リセットなので、同時に二つのフェンスで使わない。
    pub fn wait_fence(&self, fence: &ID3D12Fence, value: u64) -> Result<(), Error> {
        if completed(fence) >= value {
            return Ok(());
        }
        // SAFETY: フェンスもイベントも有効。
        unsafe { fence.SetEventOnCompletion(value, self.0) }
            .map_err(|error| failed("SetEventOnCompletion", error))?;
        // SAFETY: イベントはこのオブジェクトが持っていて、まだ開いている。
        if unsafe { WaitForSingleObject(self.0, INFINITE) } != WAIT_OBJECT_0 {
            return Err(Error::Unavailable(String::from("待機に失敗しました")));
        }
        Ok(())
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: 作ったハンドルを一度だけ閉じる。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub(crate) fn completed(fence: &ID3D12Fence) -> u64 {
    // SAFETY: 読むだけ。
    unsafe { fence.GetCompletedValue() }
}

pub(crate) fn describe(resource: &ID3D12Resource) -> D3D12_RESOURCE_DESC {
    // SAFETY: 読むだけ。
    unsafe { resource.GetDesc() }
}

pub(crate) fn luid(device: &ID3D12Device) -> LUID {
    // SAFETY: 読むだけ。
    unsafe { device.GetAdapterLuid() }
}

/// テクスチャのプレーン（二つまで）をバッファに置くときのレイアウト、行数、全体のサイズ。
pub(crate) fn footprints(
    device: &ID3D12Device,
    description: &D3D12_RESOURCE_DESC,
    planes: usize,
) -> ([D3D12_PLACED_SUBRESOURCE_FOOTPRINT; 2], [u32; 2], u64) {
    let planes = planes.min(2);
    let mut layouts = [D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); 2];
    let mut rows = [0u32; 2];
    let mut row_bytes = [0u64; 2];
    let mut total = 0u64;
    // SAFETY: 配列はどれも `planes` 個分あり、記述子は有効な借用。
    unsafe {
        device.GetCopyableFootprints(
            description,
            0,
            planes as u32,
            0,
            Some(layouts.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            Some(row_bytes.as_mut_ptr()),
            Some(&mut total),
        );
    }
    (layouts, rows, total)
}

pub(crate) fn factory<T: Interface>() -> Result<T, Error> {
    // SAFETY: 引数を取らない。
    unsafe { CreateDXGIFactory1() }.map_err(|error| failed("CreateDXGIFactory1", error))
}

pub(crate) fn adapter_by_luid<T: Interface>(luid: LUID) -> Result<T, Error> {
    let factory: IDXGIFactory4 = factory()?;
    // SAFETY: 有効なファクトリーに値を一つ渡す。
    unsafe { factory.EnumAdapterByLuid(luid) }.map_err(|error| failed("EnumAdapterByLuid", error))
}

pub(crate) fn describe_adapter(adapter: &IDXGIAdapter1) -> Option<DXGI_ADAPTER_DESC1> {
    // SAFETY: 読むだけ。
    unsafe { adapter.GetDesc1() }.ok()
}

/// ソフトウェアでないアダプターを、番号の順に `most` 個まで。
pub(crate) fn hardware_adapters(
    factory: &IDXGIFactory1,
    most: u32,
) -> Vec<(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> {
    let mut found = Vec::new();
    for index in 0..most {
        // SAFETY: アダプターが無くなると失敗するだけ。
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Some(description) = describe_adapter(&adapter) else {
            continue;
        };
        if DXGI_ADAPTER_FLAG(description.Flags as i32).0 & DXGI_ADAPTER_FLAG_SOFTWARE.0 != 0 {
            continue;
        }
        found.push((adapter, description));
    }
    found
}

pub(crate) fn create_device(adapter: &IDXGIAdapter1) -> Result<ID3D12Device, Error> {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: 有効なアダプター。`device` は一度だけ書かれる。
    unsafe { D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }
        .map_err(|error| failed("D3D12CreateDevice", error))?;
    device.ok_or_else(|| Error::Unavailable(String::from("デバイスが作れません")))
}

pub(crate) fn device_by_luid(luid: LUID) -> Result<ID3D12Device, Error> {
    create_device(&adapter_by_luid(luid)?)
}

/// CPU から見えるバッファのマップ。drop で書いた範囲を指定してアンマップする。
pub(crate) struct Mapping<'a> {
    buffer: &'a ID3D12Resource,
    base: *mut u8,
    size: usize,
    written: usize,
}

impl<'a> Mapping<'a> {
    /// `size` はバッファのサイズ、`read` は CPU が読む範囲（読まないなら空）。
    pub fn new(
        buffer: &'a ID3D12Resource,
        size: usize,
        read: std::ops::Range<usize>,
    ) -> Result<Self, Error> {
        let mut base: *mut std::ffi::c_void = std::ptr::null_mut();
        let range = D3D12_RANGE {
            Begin: read.start,
            End: read.end,
        };
        // SAFETY: サブリソース 0 はバッファの全体。`base` は有効なローカル変数。
        unsafe { buffer.Map(0, Some(&range), Some(&mut base)) }
            .map_err(|error| failed("Map", error))?;
        if base.is_null() {
            return Err(Error::Unavailable(String::from(
                "バッファをマップできません",
            )));
        }
        Ok(Self {
            buffer,
            base: base.cast(),
            size,
            written: 0,
        })
    }

    pub fn bytes(&self, range: std::ops::Range<usize>) -> &[u8] {
        assert!(range.start <= range.end && range.end <= self.size);
        // SAFETY: 範囲はマップした `size` バイトの内側で、アンマップするまで有効。
        unsafe { std::slice::from_raw_parts(self.base.add(range.start), range.len()) }
    }

    /// 先頭から `data` を書く。
    pub fn write(&mut self, data: &[u8]) {
        assert!(data.len() <= self.size);
        // SAFETY: マップした `size` バイトの内側で、重ならない。
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.base, data.len()) };
        self.written = self.written.max(data.len());
    }
}

impl Drop for Mapping<'_> {
    fn drop(&mut self) {
        let written = D3D12_RANGE {
            Begin: 0,
            End: self.written,
        };
        // SAFETY: `new` でマップしたもので、借用したスライスはもう drop されている。
        unsafe { self.buffer.Unmap(0, Some(&written)) };
    }
}
