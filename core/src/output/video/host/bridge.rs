// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 補間用 GPU とレンダリング用 GPU の受け渡し（D3D12）。同じ GPU なら共有ヒープのテクスチャをそのまま使い、
//! 別の GPU なら GPU 間で共有するバッファとフェンスを挟む。できなければメインメモリを通す。
//! `ZW_BRIDGE=host` でメインメモリ経由を指定できる。

use wgpu::hal::api::Dx12;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, LUID};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1,
};

use super::Gpu;
use super::d3d12::{self, Fence};
use super::staging::{self, Layout, Staging};

pub(super) struct Adapter {
    pub name: String,
    pub vendor: u32,
    pub luid: LUID,
    pub dxgi: IDXGIAdapter1,
}

pub(super) fn adapters() -> Vec<Adapter> {
    // SAFETY: ファクトリーは関数の中だけで使う。
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for index in 0..16u32 {
        // SAFETY: 列挙し終えるとエラーが返る。
        let Ok(dxgi) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: 読むだけ。
        let Ok(description) = (unsafe { dxgi.GetDesc1() }) else {
            continue;
        };
        if DXGI_ADAPTER_FLAG(description.Flags as i32).contains(DXGI_ADAPTER_FLAG_SOFTWARE) {
            continue;
        }
        let end = description
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(description.Description.len());
        found.push(Adapter {
            name: String::from_utf16_lossy(&description.Description[..end]),
            vendor: description.VendorId,
            luid: description.AdapterLuid,
            dxgi,
        });
    }
    found
}

fn same_luid(a: LUID, b: LUID) -> bool {
    a.LowPart == b.LowPart && a.HighPart == b.HighPart
}

pub(super) fn adapter_of(raw: &ID3D12Device) -> Result<IDXGIAdapter1, String> {
    let luid = d3d12::luid(raw);
    adapters()
        .into_iter()
        .find(|adapter| same_luid(adapter.luid, luid))
        .map(|adapter| adapter.dxgi)
        .ok_or_else(|| String::from("このデバイスの GPU が DXGI に見えない"))
}

pub(super) fn has_vendor(vendor: u32) -> bool {
    adapters().iter().any(|adapter| adapter.vendor == vendor)
}

pub(super) struct Worker {
    pub device: ID3D12Device,
    pub name: String,
    cross: Option<Cross>,
}

/// 別の GPU のときだけ：補間用 GPU のキューと、二つの GPU をつなぐフェンス。
struct Cross {
    queue: ID3D12CommandQueue,
    /// コピーのコマンドリストを順番に使い回す（一つ前のコピーの完了を CPU で待たない）。
    slots: Vec<Slot>,
    next: usize,
    done: Fence,
    /// GPU 間で共有するフェンス。作れなければメインメモリを通す。
    link: Option<Link>,
}

struct Link {
    /// レンダリング用 GPU がシグナルし、補間用 GPU が待つ（レンダリング側で作り、補間側で開いた物）。
    to_worker: (Fence, ID3D12Fence),
    to_render: (Fence, ID3D12Fence),
}

struct Slot {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    used: u64,
}

/// 1 フレームの受け渡し用のレーン。`local` は補間用 GPU のリソース（共有ヒープなので D3D11 でも開ける）。
pub(super) struct Lane {
    pub planes: Vec<wgpu::Texture>,
    pub local: ID3D12Resource,
    cross: Option<CrossBuffer>,
}

impl Lane {
    pub fn render(&self) -> &wgpu::Texture {
        &self.planes[0]
    }
}

struct CrossBuffer {
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    way: Way,
}

enum Way {
    Shared {
        render: wgpu::Buffer,
        worker: ID3D12Resource,
    },
    Host {
        staging: Staging,
        upload: ID3D12Resource,
        readback: ID3D12Resource,
    },
}

impl CrossBuffer {
    fn worker_side(&self, sending: bool) -> &ID3D12Resource {
        match &self.way {
            Way::Shared { worker, .. } => worker,
            Way::Host { upload, .. } if sending => upload,
            Way::Host { readback, .. } => readback,
        }
    }
}

impl Worker {
    /// `vendor` の GPU で処理する（レンダリング用 GPU が別のベンダーなら、そのベンダーの最初の GPU で）。
    pub fn open(render: &wgpu::Device, vendor: u32) -> Result<Self, String> {
        let render_raw = d3d12::device(render)?;
        let render_luid = d3d12::luid(&render_raw);
        let all = adapters();
        if let Some(here) = all
            .iter()
            .find(|adapter| same_luid(adapter.luid, render_luid))
            && here.vendor == vendor
        {
            return Ok(Self {
                device: render_raw,
                name: here.name.clone(),
                cross: None,
            });
        }
        let adapter = all
            .into_iter()
            .find(|adapter| adapter.vendor == vendor)
            .ok_or_else(|| format!("ベンダー {vendor:04x} の GPU が無い"))?;
        let mut device: Option<ID3D12Device> = None;
        // SAFETY: 出力先は有効なローカル変数。
        unsafe { D3D12CreateDevice(&adapter.dxgi, D3D_FEATURE_LEVEL_11_0, &mut device) }
            .map_err(|e| format!("{} で D3D12 デバイスを作れない：{e}", adapter.name))?;
        let device = device.ok_or("D3D12 デバイスが無い")?;
        // SAFETY: 有効なデバイスから作る。
        let queue: ID3D12CommandQueue = unsafe {
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
        }
        .map_err(|e| format!("CreateCommandQueue：{e}"))?;
        let slots = (0..SLOTS)
            .map(|_| -> Result<Slot, String> {
                // SAFETY: 有効なデバイスから作る。
                unsafe {
                    let allocator: ID3D12CommandAllocator = device
                        .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                        .map_err(|e| format!("CreateCommandAllocator：{e}"))?;
                    let list: ID3D12GraphicsCommandList = device
                        .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                        .map_err(|e| format!("CreateCommandList：{e}"))?;
                    list.Close().map_err(|e| format!("Close：{e}"))?;
                    Ok(Slot {
                        allocator,
                        list,
                        used: 0,
                    })
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let link =
            |owner: &ID3D12Device, other: &ID3D12Device| -> Result<(Fence, ID3D12Fence), String> {
                let fence = Fence::new_with(
                    owner,
                    D3D12_FENCE_FLAG_SHARED | D3D12_FENCE_FLAG_SHARED_CROSS_ADAPTER,
                )?;
                let opened = open_across(owner, &fence.fence, other, "GPU 間のフェンス")?;
                Ok((fence, opened))
            };
        let forced = std::env::var("ZW_BRIDGE").is_ok_and(|way| way == "host");
        let linked = if forced {
            Err(String::from("ZW_BRIDGE=host"))
        } else {
            (|| {
                Ok(Link {
                    to_worker: link(&render_raw, &device)?,
                    to_render: link(&render_raw, &device)?,
                })
            })()
        };
        let link = match linked {
            Ok(link) => Some(link),
            Err(why) => {
                tracing::warn!(
                    "補間用 GPU: GPU 間の共有フェンスを作れないのでメインメモリ経由にする（{why}）"
                );
                None
            }
        };
        let done = Fence::new(&device, false)?;
        tracing::info!(
            "補間用 GPU: {}（レンダリング用 GPU とは別、{}）",
            adapter.name,
            if link.is_some() {
                "GPU 間で共有"
            } else {
                "メインメモリ経由"
            }
        );
        Ok(Self {
            device,
            name: adapter.name,
            cross: Some(Cross {
                queue,
                slots,
                next: 0,
                done,
                link,
            }),
        })
    }

    pub fn manner(&self) -> &'static str {
        match &self.cross {
            None => "",
            Some(cross) if cross.link.is_some() => "（レンダリング用 GPU とは別、GPU 間で共有）",
            Some(_) => "（レンダリング用 GPU とは別、メインメモリ経由）",
        }
    }

    pub fn queue(&self, gpu: &Gpu<'_>) -> Result<ID3D12CommandQueue, String> {
        match &self.cross {
            Some(cross) => Ok(cross.queue.clone()),
            None => d3d12::queue(gpu.queue),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn lane(
        &self,
        render: &wgpu::Device,
        size: [u32; 2],
        format: DXGI_FORMAT,
        planes: &[(wgpu::TextureFormat, [u32; 2])],
        flags: D3D12_RESOURCE_FLAGS,
        usage: wgpu::TextureUsages,
        label: &str,
    ) -> Result<Lane, String> {
        let local = d3d12::resource(&self.device, size, format, flags, D3D12_HEAP_FLAG_SHARED)?;
        if self.cross.is_none() {
            let planes = if let [(plane_format, plane_size)] = planes {
                vec![d3d12::wrap(
                    render,
                    local.clone(),
                    *plane_size,
                    *plane_format,
                    usage,
                    label,
                )]
            } else {
                (0u32..)
                    .zip(planes)
                    .map(|(index, (plane_format, plane_size))| {
                        d3d12::wrap_plane(
                            render,
                            local.clone(),
                            index,
                            *plane_size,
                            *plane_format,
                            usage,
                            label,
                        )
                    })
                    .collect()
            };
            return Ok(Lane {
                planes,
                local,
                cross: None,
            });
        }
        let textures = planes
            .iter()
            .map(|(plane_format, plane_size)| {
                render.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: plane_size[0],
                        height: plane_size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: *plane_format,
                    usage: usage | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();
        // 行の幅は 256 バイト、プレーンの先頭は 512 バイトにそろえる（wgpu のコピーのアライメント）。
        let count = planes.len() as u32;
        let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); planes.len()];
        let mut total = 0u64;
        // SAFETY: 読むだけ。出力先は有効なローカル変数で、数は `count`。
        unsafe {
            let description = local.GetDesc();
            self.device.GetCopyableFootprints(
                &description,
                0,
                count,
                0,
                Some(footprints.as_mut_ptr()),
                None,
                None,
                Some(&mut total),
            );
        }
        let cross = self.cross.as_ref().ok_or("補間用 GPU が別の GPU でない")?;
        let shared = match cross.link.is_some() {
            true => shared_buffer(render, &self.device, total, label),
            false => Err(String::from("GPU 間で共有するフェンスが無い")),
        };
        let way = match shared {
            Ok(way) => way,
            Err(why) => {
                if cross.link.is_some() {
                    tracing::warn!("補間用 GPU: この経路はメインメモリ経由にする（{why}）");
                }
                let layout = Layout {
                    planes: footprints
                        .iter()
                        .map(|footprint| staging::Plane {
                            offset: footprint.Offset,
                            row_pitch: footprint.Footprint.RowPitch,
                            rows: footprint.Footprint.Height,
                        })
                        .collect(),
                    total,
                };
                Way::Host {
                    staging: Staging::new(render, layout, label),
                    upload: host_buffer(&self.device, total, D3D12_HEAP_TYPE_UPLOAD)?,
                    readback: host_buffer(&self.device, total, D3D12_HEAP_TYPE_READBACK)?,
                }
            }
        };
        Ok(Lane {
            planes: textures,
            local,
            cross: Some(CrossBuffer { footprints, way }),
        })
    }

    pub fn send(&mut self, gpu: &mut Gpu<'_>, lanes: &[&Lane]) -> Result<(), String> {
        let Some(cross) = self.cross.as_mut() else {
            return Ok(());
        };
        let mut hosts: Vec<(&Staging, &[wgpu::Texture], &ID3D12Resource)> = Vec::new();
        let mut any_shared = false;
        for lane in lanes {
            let Some(buffer) = &lane.cross else {
                continue;
            };
            match &buffer.way {
                Way::Shared { render, .. } => {
                    any_shared = true;
                    for (texture, footprint) in lane.planes.iter().zip(&buffer.footprints) {
                        gpu.encoder.copy_texture_to_buffer(
                            texture.as_image_copy(),
                            wgpu::TexelCopyBufferInfo {
                                buffer: render,
                                layout: layout(footprint),
                            },
                            texture.size(),
                        );
                    }
                }
                Way::Host {
                    staging, upload, ..
                } => hosts.push((staging, &lane.planes, upload)),
            }
        }
        if hosts.is_empty() {
            gpu.flush();
        } else {
            let pairs: Vec<(&Staging, &[wgpu::Texture])> = hosts
                .iter()
                .map(|(staging, planes, _)| (*staging, *planes))
                .collect();
            staging::download(gpu, &pairs, |index, bytes| {
                write_host(hosts[index].2, bytes)
            })?;
        }
        if any_shared && let Some(link) = cross.link.as_mut() {
            let render_queue = d3d12::queue(gpu.queue)?;
            let ready = link.to_worker.0.next();
            // レンダリング用 GPU がコピーし終えたらシグナルし、補間用 GPU は待つ。
            d3d12::signal(&render_queue, &link.to_worker.0.fence, ready)?;
            d3d12::wait(&cross.queue, &link.to_worker.1, ready)?;
        }
        cross.copy(|list| {
            for lane in lanes {
                let Some(buffer) = &lane.cross else {
                    continue;
                };
                for (index, footprint) in (0u32..).zip(&buffer.footprints) {
                    let (to, from) = (
                        subresource(&lane.local, index),
                        placed(buffer.worker_side(true), footprint),
                    );
                    // SAFETY: 二つのリソースは有効で、フットプリントは `local` の物。
                    unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
                }
            }
        })
    }

    /// `local` をレーンの `planes` へ。補間の処理を [`Worker::queue`] で待たせてから呼ぶ。
    pub fn receive(&mut self, gpu: &mut Gpu<'_>, lanes: &[&Lane]) -> Result<(), String> {
        let Some(cross) = self.cross.as_mut() else {
            return Ok(());
        };
        cross.copy(|list| {
            for lane in lanes {
                let Some(buffer) = &lane.cross else {
                    continue;
                };
                for (index, footprint) in (0u32..).zip(&buffer.footprints) {
                    let (to, from) = (
                        placed(buffer.worker_side(false), footprint),
                        subresource(&lane.local, index),
                    );
                    // SAFETY: 二つのリソースは有効で、フットプリントは `local` の物。
                    unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
                }
            }
        })?;
        let any_shared = lanes.iter().any(|lane| {
            matches!(
                &lane.cross,
                Some(CrossBuffer {
                    way: Way::Shared { .. },
                    ..
                })
            )
        });
        let any_host = lanes.iter().any(|lane| {
            matches!(
                &lane.cross,
                Some(CrossBuffer {
                    way: Way::Host { .. },
                    ..
                })
            )
        });
        if any_shared && let Some(link) = cross.link.as_mut() {
            let ready = link.to_render.0.next();
            let render_queue = d3d12::queue(gpu.queue)?;
            // 補間用 GPU がコピーし終えたらシグナルし、レンダリング用 GPU は待つ。
            d3d12::signal(&cross.queue, &link.to_render.1, ready)?;
            d3d12::wait(&render_queue, &link.to_render.0.fence, ready)?;
        }
        if any_host {
            cross.done.wait(cross.done.value)?;
        }
        for lane in lanes {
            let Some(buffer) = &lane.cross else {
                continue;
            };
            match &buffer.way {
                Way::Shared { render, .. } => {
                    for (texture, footprint) in lane.planes.iter().zip(&buffer.footprints) {
                        gpu.encoder.copy_buffer_to_texture(
                            wgpu::TexelCopyBufferInfo {
                                buffer: render,
                                layout: layout(footprint),
                            },
                            texture.as_image_copy(),
                            texture.size(),
                        );
                    }
                }
                Way::Host {
                    staging, readback, ..
                } => read_host(readback, |bytes| staging.upload(gpu, &lane.planes, bytes))?,
            }
        }
        Ok(())
    }
}

/// 補間の結果をレンダリング側へ転送する処理のチケット（[`Worker::fetch`]）。フェンスがこの値になれば転送済み。
pub(super) struct Ticket {
    fence: ID3D12Fence,
    value: u64,
}

impl Worker {
    /// `lanes` の `local` をレンダリング側へ転送するコマンドを補間用 GPU のキューに送る（待たない）。
    pub fn fetch(
        &mut self,
        lanes: &[&Lane],
        after: &ID3D12Fence,
        value: u64,
    ) -> Result<Ticket, String> {
        let Some(cross) = self.cross.as_mut() else {
            return Ok(Ticket {
                fence: after.clone(),
                value,
            });
        };
        d3d12::wait(&cross.queue, after, value)?;
        cross.copy(|list| {
            for lane in lanes {
                let Some(buffer) = &lane.cross else {
                    continue;
                };
                for (index, footprint) in (0u32..).zip(&buffer.footprints) {
                    let (to, from) = (
                        placed(buffer.worker_side(false), footprint),
                        subresource(&lane.local, index),
                    );
                    // SAFETY: 二つのリソースは有効で、フットプリントは `local` の物。
                    unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
                }
            }
        })?;
        let shared = lanes.iter().any(|lane| {
            matches!(
                &lane.cross,
                Some(CrossBuffer {
                    way: Way::Shared { .. },
                    ..
                })
            )
        });
        if shared && let Some(link) = cross.link.as_mut() {
            let ready = link.to_render.0.next();
            // 補間用 GPU がコピーし終えたらシグナルする。
            d3d12::signal(&cross.queue, &link.to_render.1, ready)?;
            return Ok(Ticket {
                fence: link.to_render.0.fence.clone(),
                value: ready,
            });
        }
        Ok(Ticket {
            fence: cross.done.fence.clone(),
            value: cross.done.value,
        })
    }

    pub fn fetched(&self, ticket: &Ticket) -> bool {
        d3d12::completed(&ticket.fence) >= ticket.value
    }

    /// [`Worker::fetched`] を見てから呼ぶ（レンダリング側のキューの待機は順序を守るためだけ）。
    pub fn land(
        &mut self,
        gpu: &mut Gpu<'_>,
        lanes: &[&Lane],
        ticket: &Ticket,
    ) -> Result<(), String> {
        let render_queue = d3d12::queue(gpu.queue)?;
        let host = lanes.iter().any(|lane| {
            matches!(
                &lane.cross,
                Some(CrossBuffer {
                    way: Way::Host { .. },
                    ..
                })
            )
        });
        if !host {
            // 完了済みの値なのですぐ通る。
            d3d12::wait(&render_queue, &ticket.fence, ticket.value)?;
        }
        for lane in lanes {
            let Some(buffer) = &lane.cross else {
                continue;
            };
            match &buffer.way {
                Way::Shared { render, .. } => {
                    for (texture, footprint) in lane.planes.iter().zip(&buffer.footprints) {
                        gpu.encoder.copy_buffer_to_texture(
                            wgpu::TexelCopyBufferInfo {
                                buffer: render,
                                layout: layout(footprint),
                            },
                            texture.as_image_copy(),
                            texture.size(),
                        );
                    }
                }
                Way::Host {
                    staging, readback, ..
                } => read_host(readback, |bytes| staging.upload(gpu, &lane.planes, bytes))?,
            }
        }
        Ok(())
    }
}

const SLOTS: usize = 4;

impl Cross {
    /// このスロットで前に送ったコマンドの完了を待ってから記録し直す。
    fn copy(&mut self, record: impl FnOnce(&ID3D12GraphicsCommandList)) -> Result<(), String> {
        let at = self.next;
        self.next = (at + 1) % self.slots.len();
        let slot = &mut self.slots[at];
        self.done.wait(slot.used)?;
        // SAFETY: このスロットの前のコマンドは完了している。デバイス・リスト・キューは有効。
        unsafe {
            slot.allocator
                .Reset()
                .map_err(|e| format!("CommandAllocator の Reset：{e}"))?;
            slot.list
                .Reset(&slot.allocator, None)
                .map_err(|e| format!("CommandList の Reset：{e}"))?;
            record(&slot.list);
            slot.list.Close().map_err(|e| format!("Close：{e}"))?;
            let list: ID3D12CommandList = windows::core::Interface::cast(&slot.list)
                .map_err(|e| format!("ID3D12CommandList：{e}"))?;
            self.queue.ExecuteCommandLists(&[Some(list)]);
            let done = self.done.next();
            self.queue
                .Signal(&self.done.fence, done)
                .map_err(|e| format!("Signal：{e}"))?;
            slot.used = done;
        }
        Ok(())
    }
}

fn layout(footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT) -> wgpu::TexelCopyBufferLayout {
    wgpu::TexelCopyBufferLayout {
        offset: footprint.Offset,
        bytes_per_row: Some(footprint.Footprint.RowPitch),
        rows_per_image: Some(footprint.Footprint.Height),
    }
}

fn subresource(resource: &ID3D12Resource, index: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: borrowed(resource),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: index,
        },
    }
}

fn placed(
    buffer: &ID3D12Resource,
    footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: borrowed(buffer),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: *footprint,
        },
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(cross) = &self.cross
            && let Err(why) = cross.done.wait(cross.done.value)
        {
            tracing::warn!("補間用 GPU: 終了前の待機でエラー（{why}）");
        }
    }
}

fn borrowed(resource: &ID3D12Resource) -> std::mem::ManuallyDrop<Option<ID3D12Resource>> {
    // SAFETY: 記述子はこの呼び出しの間だけ使われ、リソースはそれより長く生きる。参照カウントを増やさずにコピーする。
    unsafe { std::mem::transmute_copy(resource) }
}

/// 共有フラグ付きで作ったオブジェクトを別の GPU のデバイスで開く。
fn open_across<T: windows::core::Interface>(
    owner: &ID3D12Device,
    object: impl windows::core::Param<ID3D12DeviceChild>,
    other: &ID3D12Device,
    what: &str,
) -> Result<T, String> {
    // SAFETY: 二つのデバイスとオブジェクトは有効。ハンドルは開いたら閉じる。
    unsafe {
        let handle = owner
            .CreateSharedHandle(object, None, GENERIC_ALL.0, None)
            .map_err(|e| format!("CreateSharedHandle（{what}）：{e}"))?;
        let mut opened: Option<T> = None;
        let result = other.OpenSharedHandle(handle, &mut opened);
        let _ = CloseHandle(handle);
        result.map_err(|e| format!("OpenSharedHandle（{what}）：{e}"))?;
        opened.ok_or_else(|| format!("{what}が開けない"))
    }
}

fn shared_buffer(
    render: &wgpu::Device,
    worker: &ID3D12Device,
    total: u64,
    label: &str,
) -> Result<Way, String> {
    let render_raw = d3d12::device(render)?;
    let heap = cross_heap(&render_raw, total)?;
    let buffer = placed_buffer(&render_raw, &heap, total)?;
    let opened: ID3D12Heap = open_across(&render_raw, &heap, worker, "GPU 間のヒープ")?;
    let opened = placed_buffer(worker, &opened, total)?;
    // SAFETY: このデバイスで今作った、記述子どおりのバッファ。
    let wrapped = unsafe {
        let hal = wgpu::hal::dx12::Device::buffer_from_raw(buffer, total);
        render.create_buffer_from_hal::<Dx12>(
            hal,
            &wgpu::BufferDescriptor {
                label: Some(label),
                size: total,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )
    };
    Ok(Way::Shared {
        render: wrapped,
        worker: opened,
    })
}

fn host_buffer(
    raw: &ID3D12Device,
    size: u64,
    kind: D3D12_HEAP_TYPE,
) -> Result<ID3D12Resource, String> {
    let description = buffer_description(size, D3D12_RESOURCE_FLAG_NONE);
    let heap = D3D12_HEAP_PROPERTIES {
        Type: kind,
        ..Default::default()
    };
    let state = if kind == D3D12_HEAP_TYPE_UPLOAD {
        D3D12_RESOURCE_STATE_GENERIC_READ
    } else {
        D3D12_RESOURCE_STATE_COPY_DEST
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: 有効なローカルの記述子。
    unsafe {
        raw.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &description,
            state,
            None,
            &mut made,
        )
    }
    .map_err(|e| format!("メインメモリのバッファが作れない：{e}"))?;
    made.ok_or_else(|| String::from("メインメモリのバッファが無い"))
}

fn write_host(buffer: &ID3D12Resource, bytes: &[u8]) -> Result<(), String> {
    let mut at: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: CPU から書けるバッファで、大きさはレーンのプレーン全体（`bytes` と同じ）。
    unsafe {
        buffer
            .Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut at))
            .map_err(|e| format!("Map：{e}"))?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at.cast::<u8>(), bytes.len());
        buffer.Unmap(0, None);
    }
    Ok(())
}

fn read_host(buffer: &ID3D12Resource, each: impl FnOnce(&[u8])) -> Result<(), String> {
    let mut at: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: CPU で読めるバッファ。大きさは作ったときの物で、Unmap までだけ参照する。
    unsafe {
        let size = buffer.GetDesc().Width as usize;
        buffer
            .Map(0, None, Some(&mut at))
            .map_err(|e| format!("Map：{e}"))?;
        each(std::slice::from_raw_parts(at.cast::<u8>(), size));
        buffer.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
    }
    Ok(())
}

fn buffer_description(size: u64, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
    }
}

/// GPU 間で共有するヒープ（`SHARED_CROSS_ADAPTER`、64 KB にそろえる）。
fn cross_heap(raw: &ID3D12Device, size: u64) -> Result<ID3D12Heap, String> {
    const ALIGN: u64 = 64 * 1024;
    let description = D3D12_HEAP_DESC {
        SizeInBytes: size.div_ceil(ALIGN) * ALIGN,
        Properties: D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        },
        Alignment: 0,
        Flags: D3D12_HEAP_FLAG_SHARED | D3D12_HEAP_FLAG_SHARED_CROSS_ADAPTER,
    };
    let mut made: Option<ID3D12Heap> = None;
    // SAFETY: 有効なローカルの記述子。
    unsafe { raw.CreateHeap(&description, &mut made) }
        .map_err(|e| format!("GPU 間で共有するヒープが作れない：{e}"))?;
    made.ok_or_else(|| String::from("GPU 間で共有するヒープが無い"))
}

fn placed_buffer(
    raw: &ID3D12Device,
    heap: &ID3D12Heap,
    size: u64,
) -> Result<ID3D12Resource, String> {
    let description = buffer_description(size, D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER);
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: 有効なローカルの記述子とヒープ。
    unsafe {
        raw.CreatePlacedResource(
            heap,
            0,
            &description,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut made,
        )
    }
    .map_err(|e| format!("GPU 間で共有するバッファが作れない：{e}"))?;
    made.ok_or_else(|| String::from("GPU 間で共有するバッファが無い"))
}
