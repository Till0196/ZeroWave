// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl Device {
    /// 一枚をデコードする。ここでは待たない（待つのは [`Device::wait`]）。フェンスがその値に届くまで、その
    /// テクスチャとバッファに書かない。`twin` は参照専用のサーフェスで、状態はここで遷移させる。
    #[allow(clippy::too_many_arguments)]
    pub fn decode(
        &mut self,
        decoder: &ID3D12VideoDecoder,
        heap: &ID3D12VideoDecoderHeap,
        output: &ID3D12Resource,
        twin: Option<Twin<'_>>,
        references: &mut [Option<ID3D12Resource>],
        subresources: &mut [u32],
        bytes: &[u8],
        arguments: &[D3D12_VIDEO_DECODE_FRAME_ARGUMENT],
        query: usize,
    ) -> Result<u64, Error> {
        let Some(queue) = self.queue.clone() else {
            return Err(Error::Unavailable(String::from(
                "D3D12 のビデオデコードキューがありません",
            )));
        };
        let at = self.at;
        self.at = (self.at + 1) % self.cycle.len();
        self.wait(self.cycle[at].value)?;
        self.ensure_stats();
        self.fill(at, bytes)?;
        let (list, bitstream) = {
            let cycle = &self.cycle[at];
            let Some(bitstream) = cycle.bitstream.clone() else {
                return Err(Error::Unavailable(String::from("バッファがありません")));
            };
            let Some(list) = self.list.clone() else {
                return Err(Error::Unavailable(String::from(
                    "D3D12 のデコードのコマンドリストがありません",
                )));
            };
            // SAFETY: このアロケーターで記録した処理は終わっている（上で待った）。リストは閉じている。
            let reset = unsafe {
                cycle
                    .allocator
                    .Reset()
                    .map_err(|error| failed("Reset allocator", error))
                    .and_then(|()| {
                        list.Reset(&cycle.allocator)
                            .map_err(|error| failed("Reset list", error))
                    })
            };
            match reset {
                Ok(()) => (list, bitstream),
                Err(error) => {
                    self.renew(at);
                    return Err(error);
                }
            }
        };

        // 直接配列へデコードするときのバリアは `twin_barriers` がプレーンごとに張る（配列ごと遷移させると
        // 読む参照のスライスまで遷移する）。
        let direct = matches!(twin, Some(Twin::Direct { .. }));
        if !direct {
            barrier(
                &list,
                output,
                D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_VIDEO_DECODE_WRITE,
            );
        }
        twin_barriers(&list, output, twin, references, subresources, false);

        // インターフェイスは [`borrow`] で参照カウントを増やさずに借りる。参照ごとにデコーダーヒープも指定する
        // （解像度が変わらないので、どれも今のヒープ）。
        let mut heaps: Vec<Option<ID3D12VideoDecoderHeap>> = references
            .iter()
            .map(|reference| reference.as_ref().map(|_| heap.clone()))
            .collect();
        let count = arguments.len().min(10);
        let mut frame_arguments = [D3D12_VIDEO_DECODE_FRAME_ARGUMENT::default(); 10];
        frame_arguments[..count].copy_from_slice(&arguments[..count]);
        let input = D3D12_VIDEO_DECODE_INPUT_STREAM_ARGUMENTS {
            NumFrameArguments: count as u32,
            FrameArguments: frame_arguments,
            ReferenceFrames: D3D12_VIDEO_DECODE_REFERENCE_FRAMES {
                NumTexture2Ds: references.len() as u32,
                ppTexture2Ds: references.as_mut_ptr(),
                pSubresources: subresources.as_mut_ptr(),
                ppHeaps: heaps.as_mut_ptr(),
            },
            CompressedBitstream: D3D12_VIDEO_DECODE_COMPRESSED_BITSTREAM {
                pBuffer: borrow(&bitstream),
                Offset: 0,
                Size: bytes.len().next_multiple_of(ALIGNMENT) as u64,
            },
            pHeap: borrow(heap),
        };
        // ビデオプロセッサーで参照専用のコピーから出力する。色空間を指定しないと `0`（RGB）への変換と解釈される。
        let conversion = match twin {
            Some(Twin::Own(texture)) => Some((texture, 0)),
            Some(Twin::Slice { array, slot, .. }) => Some((array, slot)),
            Some(Twin::Direct { .. }) | None => None,
        }
        .map_or_else(
            D3D12_VIDEO_DECODE_CONVERSION_ARGUMENTS::default,
            |(texture, slot)| D3D12_VIDEO_DECODE_CONVERSION_ARGUMENTS {
                Enable: true.into(),
                pReferenceTexture2D: borrow(texture),
                ReferenceSubresource: slot,
                OutputColorSpace: DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                DecodeColorSpace: DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            },
        );
        let out = match twin {
            Some(Twin::Direct { array, slot, .. }) => D3D12_VIDEO_DECODE_OUTPUT_STREAM_ARGUMENTS {
                pOutputTexture2D: borrow(array),
                OutputSubresource: slot,
                ConversionArguments: conversion,
            },
            _ => D3D12_VIDEO_DECODE_OUTPUT_STREAM_ARGUMENTS {
                pOutputTexture2D: borrow(output),
                OutputSubresource: 0,
                ConversionArguments: conversion,
            },
        };

        // SAFETY: every pointer is to a live local or a slice the caller owns for the call.
        unsafe { list.DecodeFrame(decoder, &out, &input) };

        // ドライバーの報告を一枚ごとに取っておく。読むのは次にこのスロットを使うとき（待機が済んでいる）。
        if let Some(stats) = self.stats.as_ref()
            && (query as u32) < STAT_SLOTS
        {
            // SAFETY: the heap and buffer are this device's, the index is in range, the list open.
            unsafe {
                list.EndQuery(
                    &stats.heap,
                    D3D12_QUERY_TYPE_VIDEO_DECODE_STATISTICS,
                    query as u32,
                );
                list.ResolveQueryData(
                    &stats.heap,
                    D3D12_QUERY_TYPE_VIDEO_DECODE_STATISTICS,
                    query as u32,
                    1,
                    &stats.readback,
                    query as u64 * STAT_SIZE,
                );
            }
        }

        if !direct {
            barrier(
                &list,
                output,
                D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                D3D12_RESOURCE_STATE_VIDEO_DECODE_WRITE,
                D3D12_RESOURCE_STATE_COMMON,
            );
        }
        twin_barriers(&list, output, twin, references, subresources, true);
        // SAFETY: the list is open; the caller holds what it refers to until the value passes.
        // 閉じられないリストは再利用できないので作り直す。
        if let Err(error) = unsafe { list.Close() } {
            self.renew(at);
            return Err(failed("Close", error));
        }
        let raw: ID3D12CommandList = list
            .cast()
            .map_err(|error| failed("ID3D12CommandList", error))?;
        self.value += 1;
        let value = self.value;
        self.cycle[at].value = value;
        // 塗りつぶしが先、デコードが後。別のキューなので、このキューにコピーのフェンスを待たせる。
        if self.pending_copy > self.waited_copy
            && let Some(copy) = self.copy.as_ref()
        {
            // SAFETY: both queues and the fence are this device's; the wait is on the queue.
            unsafe { queue.Wait(&copy.fence, self.pending_copy) }
                .map_err(|error| failed("Wait", error))?;
            self.waited_copy = self.pending_copy;
        }
        // SAFETY: the list was closed above and the queue is this decoder's own.
        unsafe {
            queue.ExecuteCommandLists(&[Some(raw)]);
            queue
                .Signal(&self.fence, value)
                .map_err(|error| failed("Signal", error))?;
        }
        Ok(value)
    }

    fn ensure_stats(&mut self) -> Option<()> {
        if self.stats.is_some() {
            return Some(());
        }
        let description = D3D12_QUERY_HEAP_DESC {
            Type: D3D12_QUERY_HEAP_TYPE_VIDEO_DECODE_STATISTICS,
            Count: STAT_SLOTS,
            NodeMask: 0,
        };
        let mut heap: Option<ID3D12QueryHeap> = None;
        // SAFETY: the description is a live local; the out parameter is written at most once.
        unsafe { self.raw.CreateQueryHeap(&description, &mut heap) }.ok()?;
        let heap = heap?;
        let readback = self
            .buffer(
                u64::from(STAT_SLOTS) * STAT_SIZE,
                D3D12_HEAP_TYPE_READBACK,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
            .ok()?;
        self.stats = Some(Stats { heap, readback });
        Some(())
    }

    /// ドライバーがこのスロットのフレームについて報告した内容。そのフレームのフェンスを待った後でだけ呼ぶ。
    pub fn told(&self, slot: usize) -> Option<u64> {
        let stats = self.stats.as_ref()?;
        if slot as u32 >= STAT_SLOTS {
            return None;
        }
        let offset = (slot as u64 * STAT_SIZE) as usize;
        let range = offset..offset + STAT_SIZE as usize;
        let mapping = Mapping::new(
            &stats.readback,
            (u64::from(STAT_SLOTS) * STAT_SIZE) as usize,
            range.clone(),
        )
        .ok()?;
        let answer = statistics(mapping.bytes(range));
        drop(mapping);
        if answer.Status == RATE_EXCEEDED
            && !RATE_SAID.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            tracing::warn!(
                "D3D12: ドライバーが BitRate と FrameRate の不足を報告（必要 {} bit/s、{}/{} fps）",
                answer.BitRate,
                answer.FrameRate.Numerator,
                answer.FrameRate.Denominator,
            );
        }
        Some(answer.Status)
    }

    pub fn told_to_skip(&self, slot: usize) -> bool {
        self.told(slot) == Some(SKIP_DISPLAY)
    }

    /// 一枚のビットストリームをこの項目のバッファへ入れる（入らなければ大きく作り直す）。
    fn fill(&mut self, at: usize, bytes: &[u8]) -> Result<(), Error> {
        let padded = bytes.len().next_multiple_of(ALIGNMENT);
        if self.cycle[at].capacity < padded as u64 {
            let want = (padded as u64).next_power_of_two().max(1 << 20);
            let buffer = self.upload(want)?;
            name(&buffer, &format!("zerowave ビットストリーム{at}"));
            self.cycle[at].bitstream = Some(buffer);
            self.cycle[at].capacity = want;
        }
        let Some(buffer) = self.cycle[at].bitstream.clone() else {
            return Err(Error::Unavailable(String::from("バッファがありません")));
        };
        let mut whole = Vec::with_capacity(padded);
        whole.extend_from_slice(bytes);
        whole.resize(padded, 0);
        self.write(&buffer, &whole)
    }

    pub(super) fn video_queue(
        raw: &ID3D12Device,
    ) -> Result<(ID3D12CommandQueue, ID3D12VideoDecodeCommandList, Vec<Cycle>), Error> {
        let queue = shared_queue(raw)?;
        let mut cycle = Vec::with_capacity(CYCLE);
        for _ in 0..CYCLE {
            cycle.push(Cycle {
                allocator: allocator(raw, D3D12_COMMAND_LIST_TYPE_VIDEO_DECODE)?,
                bitstream: None,
                capacity: 0,
                value: 0,
            });
        }
        let list = Self::list(raw, &cycle[0].allocator)?;
        Ok((queue, list, cycle))
    }

    /// 閉じたリスト（最初の `Reset` で開く）。
    fn list(
        raw: &ID3D12Device,
        allocator: &ID3D12CommandAllocator,
    ) -> Result<ID3D12VideoDecodeCommandList, Error> {
        // SAFETY: 生きているデバイスと、このリストの種類のアロケーター。
        unsafe {
            let list: ID3D12VideoDecodeCommandList = raw
                .CreateCommandList(
                    0,
                    D3D12_COMMAND_LIST_TYPE_VIDEO_DECODE,
                    allocator,
                    None::<&ID3D12PipelineState>,
                )
                .map_err(|error| failed("CreateCommandList", error))?;
            list.Close().map_err(|error| failed("Close", error))?;
            name(&list, "zerowave デコードのコマンドリスト");
            Ok(list)
        }
    }

    /// リストとこの項目のアロケーターを作り直す。閉じられなかったリストとそのアロケーターは再利用できない。
    fn renew(&mut self, at: usize) {
        let made = allocator(&self.raw, D3D12_COMMAND_LIST_TYPE_VIDEO_DECODE)
            .and_then(|allocator| Self::list(&self.raw, &allocator).map(|list| (allocator, list)));
        match made {
            Ok((allocator, list)) => {
                self.cycle[at].allocator = allocator;
                self.list = Some(list);
            }
            Err(error) => tracing::warn!("D3D12: コマンドリストを作り直せない: {error:?}"),
        }
    }

    /// 何もデコードしていないテクスチャを黒（色差は中間値）で埋める。0 だと真緑に出る。
    pub fn blacken(&mut self, texture: &ID3D12Resource, planes: usize) -> Result<(), Error> {
        let black = if self.ten_bit(texture) {
            // P010 は上位ビットに寄せる：黒 64 は 0x1000、中間値 512 は 0x8000。下位バイトから。
            [[0x00, 0x10], [0x00, 0x80]]
        } else {
            [[16, 16], [128, 128]]
        };
        let whole = [0, 1];
        self.wash(texture, &whole[..planes.min(2)], &black)
    }

    /// 規格 8.3.3 の代わりのピクチャーの値 `1 << (BitDepth - 1)` で埋める（輝度も色差も中間値）。
    /// `planes` は輝度と色差のサブリソース（配列のスライスなら `s` と `s + 枚数`）。
    pub fn gray(&mut self, texture: &ID3D12Resource, planes: [u32; 2]) -> Result<(), Error> {
        let middle = if self.ten_bit(texture) {
            // P010 の 512 は上位ビットに寄せて 0x8000。下位バイトから。
            [[0x00, 0x80], [0x00, 0x80]]
        } else {
            [[128, 128], [128, 128]]
        };
        self.wash(texture, &planes, &middle)
    }

    fn ten_bit(&self, texture: &ID3D12Resource) -> bool {
        describe(texture).Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_P010
    }

    /// テクスチャのプレーン（`planes` はそのサブリソース）を 2 バイトの繰り返しで埋める。参照専用の
    /// テクスチャはコピー先にできない（`Close` が `E_INVALIDARG` を返し、アロケーターも使えなくなる）。
    fn wash(
        &mut self,
        texture: &ID3D12Resource,
        planes: &[u32],
        bytes: &[[u8; 2]; 2],
    ) -> Result<(), Error> {
        match self.wash_once(texture, planes, bytes) {
            Ok(()) => Ok(()),
            Err(error) => {
                // 失敗したコマンドリストは、以後どの `Reset` も通らない。
                self.copy = None;
                self.pending_copy = 0;
                self.waited_copy = 0;
                Err(error)
            }
        }
    }

    fn wash_once(
        &mut self,
        texture: &ID3D12Resource,
        planes: &[u32],
        bytes: &[[u8; 2]; 2],
    ) -> Result<(), Error> {
        self.ensure_copy()?;
        // 配列のスライスはどれも同じレイアウトなので、一枚分の記述で数える。
        let mut one = describe(texture);
        one.DepthOrArraySize = 1;
        let (layouts, _, total) = footprints(&self.raw, &one, planes.len());
        let buffer = self.upload(total)?;
        let split = if planes.len() > 1 {
            (layouts[1].Offset as usize).min(total as usize)
        } else {
            total as usize
        };
        let mut filling = vec![0u8; total as usize];
        for (at, byte) in filling.iter_mut().enumerate() {
            let (pattern, from) = if at < split {
                (bytes[0], 0)
            } else {
                (bytes[1], split)
            };
            *byte = pattern[(at - from) % 2];
        }
        self.write(&buffer, &filling)?;
        let Some(copy) = self.copy.as_mut() else {
            return Err(Error::Unavailable(String::from("コピーキューがありません")));
        };
        copy.begin(None)?;
        for (layout, plane) in layouts.iter().zip(planes) {
            copy.region(&subresource(texture, *plane), &placed(&buffer, *layout));
        }
        let value = copy.submit()?;
        // 待たない。フェンスの値を覚え、次の `DecodeFrame` のキューに待たせる（[`Device::decode`]）。
        copy.holding.push((value, buffer));
        self.pending_copy = value;
        Ok(())
    }

    pub fn wait_idle(&mut self) {
        if let Err(error) = self.wait(self.value) {
            tracing::warn!("D3D12: デコードキューの待機に失敗: {error:?}");
        }
        // 使い回すキューは、ほかのデコーダーの分も含めて空にする。
        if self.drains
            && let Some(queue) = self.queue.as_ref()
        {
            self.value += 1;
            // SAFETY: 生きているキューとフェンスでシグナルを投入するだけ。
            let signalled = unsafe { queue.Signal(&self.fence, self.value) };
            if signalled.is_err() || self.wait(self.value).is_err() {
                tracing::warn!("D3D12: デコードキューを空にできない");
            }
        }
        let value = self.copy.as_ref().map_or(0, |copy| copy.value);
        if value != 0
            && let Err(error) = self.wait_copy(value)
        {
            tracing::warn!("D3D12: コピーキューの待機に失敗: {error:?}");
        }
    }
}

/// デバイスのデコードのキュー。[`one_decode_queue`] の GPU では、デバイスで一本だけ作って使い回す。
fn shared_queue(raw: &ID3D12Device) -> Result<ID3D12CommandQueue, Error> {
    struct Shared(usize, ID3D12CommandQueue);
    static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
    let one = one_decode_queue(raw);
    let mut kept = SHARED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if one
        && let Some(Shared(device, queue)) = kept.as_ref()
        && *device == raw.as_raw() as usize
    {
        return Ok(queue.clone());
    }
    let description = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_VIDEO_DECODE,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    // SAFETY: `description` is a live local and the device is alive.
    let queue: ID3D12CommandQueue = unsafe { raw.CreateCommandQueue(&description) }
        .map_err(|error| failed("CreateCommandQueue", error))?;
    name(&queue, "zerowave デコードキュー");
    if one {
        tracing::info!("D3D12: デコードのキューを一本にして使い回す");
        *kept = Some(Shared(raw.as_raw() as usize, queue.clone()));
    }
    Ok(queue)
}

/// デコードのキューを複数作ると落ちる Intel の GPU（UMA で、機能レベルが 12.0 以下か、名前に「HD」が入る）。
pub(super) fn one_decode_queue(raw: &ID3D12Device) -> bool {
    let Ok(adapter) = adapter_by_luid::<IDXGIAdapter1>(luid(raw)) else {
        return false;
    };
    let Some(desc) = describe_adapter(&adapter) else {
        return false;
    };
    if desc.VendorId != 0x8086 {
        return false;
    }
    let mut architecture = D3D12_FEATURE_DATA_ARCHITECTURE::default();
    let levels = [
        windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_2,
        windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_1,
        windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0,
        windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_1,
        windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0,
    ];
    let mut supported = D3D12_FEATURE_DATA_FEATURE_LEVELS {
        NumFeatureLevels: levels.len() as u32,
        pFeatureLevelsRequested: levels.as_ptr(),
        MaxSupportedFeatureLevel: windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0,
    };
    // SAFETY: 大きさどおりのローカルを渡す。
    let (uma, level) = unsafe {
        let uma = raw
            .CheckFeatureSupport(
                D3D12_FEATURE_ARCHITECTURE,
                std::ptr::from_mut(&mut architecture).cast(),
                size_of::<D3D12_FEATURE_DATA_ARCHITECTURE>() as u32,
            )
            .is_ok()
            && architecture.UMA.as_bool();
        let level = raw
            .CheckFeatureSupport(
                D3D12_FEATURE_FEATURE_LEVELS,
                std::ptr::from_mut(&mut supported).cast(),
                size_of::<D3D12_FEATURE_DATA_FEATURE_LEVELS>() as u32,
            )
            .map(|()| supported.MaxSupportedFeatureLevel)
            .unwrap_or(windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0);
        (uma, level)
    };
    let name = String::from_utf16_lossy(&desc.Description);
    uma && (level.0 <= windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0.0
        || name.contains("HD"))
}

/// `D3D12_QUERY_DATA_VIDEO_DECODE_STATISTICS` を読む（`Status`・`NumMacroblocksAffected`・
/// `FrameRate`・`BitRate` の順に 8・8・4+4・4 バイト）。
fn statistics(bytes: &[u8]) -> D3D12_QUERY_DATA_VIDEO_DECODE_STATISTICS {
    let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or([0; 8]));
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]));
    D3D12_QUERY_DATA_VIDEO_DECODE_STATISTICS {
        Status: u64_at(0),
        NumMacroblocksAffected: u64_at(8),
        FrameRate: DXGI_RATIONAL {
            Numerator: u32_at(16),
            Denominator: u32_at(20),
        },
        BitRate: u32_at(24),
    }
}

/// 参照専用の側のバリア。配列ではスライス `s` のプレーン `p` はサブリソース `s + p * count`。
fn twin_barriers(
    list: &ID3D12VideoDecodeCommandList,
    output: &ID3D12Resource,
    twin: Option<Twin<'_>>,
    references: &[Option<ID3D12Resource>],
    subresources: &[u32],
    back: bool,
) {
    let (from, write, read) = if back {
        (
            D3D12_RESOURCE_STATE_VIDEO_DECODE_WRITE,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COMMON,
        )
    } else {
        (
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_VIDEO_DECODE_WRITE,
            D3D12_RESOURCE_STATE_VIDEO_DECODE_READ,
        )
    };
    let read_from = if back {
        D3D12_RESOURCE_STATE_VIDEO_DECODE_READ
    } else {
        D3D12_RESOURCE_STATE_COMMON
    };
    match twin {
        // 参照は渡すテクスチャそのもの。デコード中のもの（書き込みの状態）は除く。
        None => {
            for reference in references.iter().flatten() {
                if same(reference, output) {
                    continue;
                }
                barrier(
                    list,
                    reference,
                    D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    read_from,
                    read,
                );
            }
        }
        Some(Twin::Own(texture)) => {
            barrier(
                list,
                texture,
                D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                from,
                write,
            );
            for reference in references.iter().flatten() {
                // 書いているコピーは読み取り状態へ遷移させない（H.264 の二つ目のフィールドは同じテクスチャの
                // 一つ目を参照する）。両方へ遷移させるとドライバーはエラー無しにそのピクチャーを書かない。
                if same(reference, texture) {
                    continue;
                }
                barrier(
                    list,
                    reference,
                    D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    read_from,
                    read,
                );
            }
        }
        Some(Twin::Slice { array, slot, count } | Twin::Direct { array, slot, count }) => {
            for plane in 0..PLANES {
                barrier(list, array, slot + plane * count, from, write);
            }
            for (reference, slice) in references.iter().zip(subresources) {
                if reference.is_none() {
                    continue;
                }
                if *slice == slot {
                    continue;
                }
                for plane in 0..PLANES {
                    barrier(list, array, slice + plane * count, read_from, read);
                }
            }
        }
    }
}

const PLANES: u32 = 2;

pub(super) fn same(one: &ID3D12Resource, other: &ID3D12Resource) -> bool {
    one.as_raw() == other.as_raw()
}

fn barrier(
    list: &ID3D12VideoDecodeCommandList,
    resource: &ID3D12Resource,
    subresource: u32,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: borrow(resource),
                Subresource: subresource,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    };
    // SAFETY: a live local naming a resource the caller holds; the list is open; nothing kept.
    unsafe { list.ResourceBarrier(std::slice::from_ref(&barrier)) };
}
