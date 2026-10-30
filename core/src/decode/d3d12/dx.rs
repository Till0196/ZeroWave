// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコーダーが使う Direct3D 12 のオブジェクト。ドライバーを呼ぶのはここだけ。wgpu が開いたデバイスがあれば
//! それを使い（[`adopt`]）、無いビルド（テスト、`--check`）は自前のデバイスを開く。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, IDXGIAdapter1, IDXGIFactory1, IDXGIFactory6,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::core::{GUID, Interface};

use crate::decode::Error;

mod adapter;
mod copy;
mod decode;
mod kit;
mod open;

pub(crate) use adapter::same_luid;
pub use adapter::{
    Adapter, adapters, draws_on, enable_child_probe, enable_dred, name, probe_child, probe_video,
    report_removed,
};
pub use copy::profile_name;
pub(crate) use kit::{
    Mapping, Owned, adapter_by_luid, buffer, completed, create_device, describe, describe_adapter,
    device_by_luid, factory, footprints, hardware_adapters, heap, luid, shared_buffer, whole_rows,
};
pub(crate) use open::texture;

/// wgpu が開いたデバイス。`decode::open` はコーデックしか受け取らないので `static` に置く。
static ADOPTED: Mutex<Option<ID3D12Device>> = Mutex::new(None);

/// このプロセスのデコーダーが使うデバイスを登録する。レンダラーがデバイスを作ったらすぐ、デコードより前に呼ぶ。
pub fn adopt(device: &ID3D12Device) {
    if let Ok(mut slot) = ADOPTED.lock() {
        *slot = Some(device.clone());
    }
}

/// 覚えているデバイスを手放す。失ったデバイスへの参照が残っていると、D3D12 は同じアダプターのデバイスとして
/// それを返し続け、作り直せない。
pub fn forget() {
    if let Ok(mut slot) = ADOPTED.lock() {
        *slot = None;
    }
}

pub fn adopted() -> Option<ID3D12Device> {
    ADOPTED.lock().ok().and_then(|slot| slot.clone())
}

static REFUSED: AtomicBool = AtomicBool::new(false);

/// このプロセスは D3D12 以外でレンダリングすることを登録する。そのときはデコードもソフトウェア。
pub fn refuse() {
    REFUSED.store(true, Ordering::Release);
    if let Ok(mut slot) = ADOPTED.lock() {
        *slot = None;
    }
}

/// ドライバーのエラーを [`Error::Unavailable`] にする（ソフトウェアにフォールバックするだけで、故障ではない）。
pub(crate) fn failed(what: &str, error: windows::core::Error) -> Error {
    if removed(error.code()) {
        tell_renderer(what);
    }
    Error::Unavailable(format!("{what}: {error}"))
}

/// デバイスが外されたときの HRESULT（`DXGI_ERROR_DEVICE_REMOVED`・`HUNG`・`RESET`）。
fn removed(code: windows::core::HRESULT) -> bool {
    use windows::Win32::Graphics::Dxgi::{
        DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    };
    matches!(
        code,
        DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_HUNG | DXGI_ERROR_DEVICE_RESET
    )
}

/// 外されたのがレンダリングと共有しているデバイスなら、レンダリングの側に開き直させる
/// （別のアダプターで開いた自前のデバイスなら、共有しているデバイスは外されていない）。
fn tell_renderer(what: &str) {
    #[cfg(feature = "wgpu")]
    if let Some(device) = adopted()
        // SAFETY: the device is alive; the call only reads its state.
        && unsafe { device.GetDeviceRemovedReason() }.is_err()
    {
        report_removed();
        crate::output::video::gpu::mark_removed(what);
    }
    #[cfg(not(feature = "wgpu"))]
    let _ = what;
}

/// ドライバーへ渡すだけの構造体のための、参照カウントを増やさないインターフェイスのコピー。
pub(crate) fn borrow<T: Interface>(from: &T) -> std::mem::ManuallyDrop<Option<T>> {
    // SAFETY: an interface is one non-null pointer and the copy is never dropped.
    unsafe { std::mem::transmute_copy(from) }
}

pub(crate) fn allocator(
    raw: &ID3D12Device,
    kind: D3D12_COMMAND_LIST_TYPE,
) -> Result<ID3D12CommandAllocator, Error> {
    // SAFETY: 生きているデバイス。
    unsafe { raw.CreateCommandAllocator(kind) }
        .map_err(|error| failed("CreateCommandAllocator", error))
}

/// リソース全体の状態遷移のバリア（参照カウントは増やさない、[`borrow`]）。
pub(crate) fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: borrow(resource),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

#[derive(Debug)]
pub struct Readback {
    pub planes: Vec<(Vec<u8>, usize, usize)>,
}

#[derive(Debug, Clone, Copy)]
pub struct Terms {
    pub configuration: D3D12_VIDEO_DECODE_CONFIGURATION,
    pub tier: D3D12_VIDEO_DECODE_TIER,
    /// 参照を `VIDEO_DECODE_REFERENCE_ONLY` の別のリソースから読むか（AMD が求める）。
    /// そのときフレームは二度書く（参照専用のコピーと、ビデオプロセッサーを通した wgpu 用のテクスチャ）。
    pub reference_only: bool,
    /// 参照を一つのテクスチャ配列のスライスにするか（Tier 1）。構成は Tier だけで決める。
    pub arrayed: bool,
    pub rows_of_32: bool,
    pub flags: i32,
}

#[derive(Debug, Clone, Copy)]
pub enum Twin<'a> {
    Own(&'a ID3D12Resource),
    Slice {
        array: &'a ID3D12Resource,
        slot: u32,
        count: u32,
    },
    /// Tier 1 で参照専用を求められないとき：配列のスライスへ直接書き、渡すフレームへはコピーキューでコピーする。
    Direct {
        array: &'a ID3D12Resource,
        slot: u32,
        count: u32,
    },
}

/// デバイス、ビデオデコードキュー、そのコマンドリスト。フェンスとイベントはキューのものなので `Clone` しない。
pub struct Device {
    pub raw: ID3D12Device,
    /// D3D12 のビデオデコード。対応しないドライバーでは無く、その GPU は D3D11VA だけでデコードする。
    pub video: Option<ID3D12VideoDevice>,
    pub shared: bool,
    pub adapter: String,
    queue: Option<ID3D12CommandQueue>,
    /// デコードのキューをほかのデコーダーと使い回している（[`decode::one_decode_queue`]）。止めるときはキューごと空にする。
    drains: bool,
    pub video_error: Option<String>,
    /// デコードのコマンドリスト。一本をアロケーターを替えて使う。
    /// 項目ごとに別のリストで記録すると、Intel の GPU ではピクチャーが書かれないことがある。
    list: Option<ID3D12VideoDecodeCommandList>,
    cycle: Vec<Cycle>,
    at: usize,
    fence: ID3D12Fence,
    event: Owned,
    value: u64,
    copy: Option<CopyQueue>,
    pending_copy: u64,
    waited_copy: u64,
    stats: Option<Stats>,
}

const ALIGNMENT: usize = 128;

/// 同時にデコードできるピクチャーの数。多すぎるとそれぞれの割り当てがメモリーを消費する。
const CYCLE: usize = 4;

struct Stats {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
}

const STAT_SIZE: u64 = 32;

const STAT_SLOTS: u32 = 128;

/// `D3D12_VIDEO_DECODE_STATUS_CONTINUE_SKIP_DISPLAY`：デコードできたが表示しない（何も書かれない）。
const SKIP_DISPLAY: u64 = 2;
const RATE_EXCEEDED: u64 = 4;
static RATE_SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Cycle {
    allocator: ID3D12CommandAllocator,
    /// このピクチャーのビットストリームのバッファ。割り当てのコストが高いので、大きくして再利用する。
    bitstream: Option<ID3D12Resource>,
    capacity: u64,
    value: u64,
}

/// コピーキューと、その一本のコマンドリスト。前のコピーを待ってから次を積む。
pub(crate) struct CopyQueue {
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    value: u64,
    /// 専用のイベント。自動リセットのイベントを二つのフェンスで共用すると、待機が素通りする。
    event: Owned,
    holding: Vec<(u64, ID3D12Resource)>,
}

impl CopyQueue {
    pub(crate) fn new(raw: &ID3D12Device) -> Result<Self, Error> {
        // SAFETY: the description is a live local; the device outlives the objects.
        unsafe {
            let queue: ID3D12CommandQueue = raw
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_COPY,
                    ..Default::default()
                })
                .map_err(|error| failed("CreateCommandQueue", error))?;
            name(&queue, "zerowave コピーキュー");
            let allocator: ID3D12CommandAllocator = raw
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)
                .map_err(|error| failed("CreateCommandAllocator", error))?;
            let list: ID3D12GraphicsCommandList = raw
                .CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_COPY,
                    &allocator,
                    None::<&ID3D12PipelineState>,
                )
                .map_err(|error| failed("CreateCommandList", error))?;
            list.Close().map_err(|error| failed("Close", error))?;
            let fence: ID3D12Fence = raw
                .CreateFence(0, D3D12_FENCE_FLAG_NONE)
                .map_err(|error| failed("CreateFence", error))?;
            Ok(Self {
                queue,
                allocator,
                list,
                fence,
                value: 0,
                event: Owned::event()?,
                holding: Vec::new(),
            })
        }
    }

    fn settle(&mut self) -> Result<(), Error> {
        self.wait(self.value)?;
        self.holding.clear();
        Ok(())
    }

    pub(crate) fn wait(&self, value: u64) -> Result<(), Error> {
        self.event.wait_fence(&self.fence, value)
    }

    /// 前のコピーを待ってからコマンドリストを開く。`after` があれば、キューにそのフェンスを先に待たせる。
    pub(crate) fn begin(&mut self, after: Option<(&ID3D12Fence, u64)>) -> Result<(), Error> {
        self.settle()?;
        if let Some((fence, value)) = after {
            // SAFETY: フェンスもキューもこのデバイスのもの。待機はキューに積まれる。
            unsafe { self.queue.Wait(fence, value) }.map_err(|error| failed("Wait", error))?;
        }
        // SAFETY: `settle` がこのアロケーターとリストの処理を全部待った。
        unsafe {
            self.allocator
                .Reset()
                .map_err(|error| failed("Reset allocator", error))?;
            self.list
                .Reset(&self.allocator, None)
                .map_err(|error| failed("Reset list", error))
        }
    }

    /// 開いたリストにコピーを一つ積む。コピーキューは `COMMON` を自動で昇格・減衰させるのでバリアは要らない。
    pub(crate) fn region(
        &self,
        destination: &D3D12_TEXTURE_COPY_LOCATION,
        source: &D3D12_TEXTURE_COPY_LOCATION,
    ) {
        // SAFETY: 二つの位置は呼び出し側が持つリソースを指し、リストは開いている。
        unsafe {
            self.list
                .CopyTextureRegion(destination, 0, 0, 0, source, None)
        };
    }

    /// テクスチャのプレーンを、`layouts` の配置でバッファへ。
    pub(crate) fn planes_into(
        &self,
        texture: &ID3D12Resource,
        buffer: &ID3D12Resource,
        layouts: &[D3D12_PLACED_SUBRESOURCE_FOOTPRINT],
    ) {
        for (plane, layout) in layouts.iter().enumerate() {
            self.region(
                &placed(buffer, *layout),
                &subresource(texture, plane as u32),
            );
        }
    }

    /// リストを閉じて実行し、終わると到達するフェンスの値を返す。
    pub(crate) fn submit(&mut self) -> Result<u64, Error> {
        // SAFETY: リストは開いていて、`begin` 以降に積んだコピーだけを持つ。
        unsafe { self.list.Close() }.map_err(|error| failed("Close", error))?;
        let list: ID3D12CommandList = self
            .list
            .cast()
            .map_err(|error| failed("ID3D12CommandList", error))?;
        self.value += 1;
        let value = self.value;
        // SAFETY: 閉じたリストを実行し、同じキューでその後にフェンスをシグナルする。
        unsafe {
            self.queue.ExecuteCommandLists(&[Some(list)]);
            self.queue
                .Signal(&self.fence, value)
                .map_err(|error| failed("Signal", error))?;
        }
        Ok(value)
    }
}

/// テクスチャのサブリソース（プレーン・配列のスライス）を指すコピーの位置。
pub(crate) fn subresource(resource: &ID3D12Resource, index: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: borrow(resource),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: index,
        },
    }
}

/// バッファの中の、一プレーン分の配置を指すコピーの位置。
pub(crate) fn placed(
    buffer: &ID3D12Resource,
    layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: borrow(buffer),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: layout,
        },
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Device(shared: {})", self.shared)
    }
}

// SAFETY: D3D12 interfaces are free threaded; a decoder is owned by one thread at a time.
unsafe impl Send for Device {}
