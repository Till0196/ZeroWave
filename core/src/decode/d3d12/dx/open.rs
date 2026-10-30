// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デバイスを開く、機能を問い合わせる、デコーダー・テクスチャ・バッファを作る。

use super::*;

impl Device {
    pub fn open() -> Result<Self, Error> {
        let (raw, shared) = match adopted() {
            Some(device) => (device, true),
            None if REFUSED.load(Ordering::Acquire) => {
                return Err(Error::Unavailable(String::from(
                    "レンダリングが D3D12 以外なので、デコードもソフトウェアで行います",
                )));
            }
            None => (Self::open_raw()?, false),
        };
        Self::build(raw, shared, String::new())
    }

    /// 名前で指定した GPU で開く。レンダリングに使う GPU ならそのデバイスを共用し、そうでなければ専用のデバイスを作る。
    pub fn open_on(name: &str) -> Result<Self, Error> {
        if name.is_empty() {
            return Self::open();
        }
        let wanted = adapters()
            .into_iter()
            .find(|adapter| adapter.name == name)
            .ok_or_else(|| Error::Unavailable(format!("{name} が見つかりません")))?;
        if let Some(device) = adopted()
            && same_luid(luid(&device), wanted.luid)
        {
            return Self::build(device, true, wanted.name);
        }
        let raw = device_by_luid(wanted.luid)?;
        Self::build(raw, false, wanted.name)
    }

    pub fn asking(adapter: &Adapter) -> Option<Self> {
        let raw = device_by_luid(adapter.luid).ok()?;
        let video: Option<ID3D12VideoDevice> = raw.cast().ok();
        // SAFETY: the device is alive.
        let fence: ID3D12Fence = unsafe { raw.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
        let event = Owned::event().ok()?;
        Some(Self {
            raw,
            video,
            shared: false,
            adapter: adapter.name.clone(),
            queue: None,
            drains: false,
            video_error: Some(String::from("問い合わせ用のデバイス")),
            list: None,
            cycle: Vec::new(),
            at: 0,
            fence,
            event,
            value: 0,
            copy: None,
            pending_copy: 0,
            stats: None,
            waited_copy: 0,
        })
    }

    fn build(raw: ID3D12Device, shared: bool, adapter: String) -> Result<Self, Error> {
        // ビデオデコードに対応しないドライバーでもデバイスは作る（D3D11VA でデコードする）。
        let video: Option<ID3D12VideoDevice> = raw.cast().ok();
        // ビデオデコードキューは、使い捨てのデバイスで作れると確かめてから作る（[`probe_video`]）。
        let made = match &video {
            Some(_) => probe_video(&raw).and_then(|()| Self::video_queue(&raw)),
            None => Err(no_video()),
        };
        let (queue, list, cycle, video_error) = match made {
            Ok((queue, list, cycle)) => (Some(queue), Some(list), cycle, None),
            Err(error) => {
                tracing::warn!("D3D12: ビデオデコードを使えない（{error}）");
                (None, None, Vec::new(), Some(error.to_string()))
            }
        };
        // SAFETY: the device is alive for the whole of this function.
        let fence: ID3D12Fence = unsafe { raw.CreateFence(0, D3D12_FENCE_FLAG_NONE) }
            .map_err(|error| failed("CreateFence", error))?;
        let event = Owned::event()?;
        let drains = queue.is_some() && decode::one_decode_queue(&raw);
        Ok(Self {
            raw,
            video,
            shared,
            adapter,
            drains,
            queue,
            video_error,
            list,
            cycle,
            at: 0,
            fence,
            event,
            value: 0,
            copy: None,
            pending_copy: 0,
            stats: None,
            waited_copy: 0,
        })
    }

    fn open_raw() -> Result<ID3D12Device, Error> {
        let factory: IDXGIFactory1 = factory()?;
        let mut chosen: Option<IDXGIAdapter1> = None;
        if let Ok(sixth) = factory.cast::<IDXGIFactory6>() {
            // SAFETY: the factory is alive for the call.
            if let Ok(adapter) = unsafe {
                sixth.EnumAdapterByGpuPreference::<IDXGIAdapter1>(
                    0,
                    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
                )
            } {
                chosen = Some(adapter);
            }
        }
        if chosen.is_none() {
            chosen = hardware_adapters(&factory, 8)
                .into_iter()
                .next()
                .map(|(adapter, _)| adapter);
        }
        let adapter =
            chosen.ok_or_else(|| Error::Unavailable(String::from("アダプターがありません")))?;
        create_device(&adapter)
    }

    /// このサイズのこのコーデックをドライバーがデコードできるか、どんな条件でか。
    pub fn supports(
        &self,
        profile: GUID,
        format: DXGI_FORMAT,
        width: u32,
        height: u32,
        first: D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE,
    ) -> Result<Terms, Error> {
        let second = if first == D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_NONE {
            D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_FIELD_BASED
        } else {
            D3D12_VIDEO_FRAME_CODED_INTERLACE_TYPE_NONE
        };
        let video = self.video()?;
        for interlace in [first, second] {
            let configuration = D3D12_VIDEO_DECODE_CONFIGURATION {
                DecodeProfile: profile,
                BitstreamEncryption: D3D12_BITSTREAM_ENCRYPTION_TYPE_NONE,
                InterlaceType: interlace,
            };
            let mut support = D3D12_FEATURE_DATA_VIDEO_DECODE_SUPPORT {
                NodeIndex: 0,
                Configuration: configuration,
                Width: width,
                Height: height,
                DecodeFormat: format,
                // 0 は「不明」（理由はデコーダーヒープの記述子のところ）。
                FrameRate: DXGI_RATIONAL {
                    Numerator: 0,
                    Denominator: 1,
                },
                BitRate: 0,
                ..Default::default()
            };
            // SAFETY: a live local of exactly the size given; the call keeps nothing.
            let asked = unsafe {
                video.CheckFeatureSupport(
                    D3D12_FEATURE_VIDEO_DECODE_SUPPORT,
                    std::ptr::from_mut(&mut support).cast(),
                    size_of::<D3D12_FEATURE_DATA_VIDEO_DECODE_SUPPORT>() as u32,
                )
            };
            if asked.is_err()
                || !support
                    .SupportFlags
                    .contains(D3D12_VIDEO_DECODE_SUPPORT_FLAG_SUPPORTED)
            {
                continue;
            }
            let arrayed = support.DecodeTier.0 < D3D12_VIDEO_DECODE_TIER_2.0;
            return Ok(Terms {
                configuration,
                tier: support.DecodeTier,
                // ドライバーが求めたときだけ参照専用にする（フラグが無ければビデオプロセッサーを使ってはいけない）。
                reference_only: support.ConfigurationFlags.contains(
                    D3D12_VIDEO_DECODE_CONFIGURATION_FLAG_REFERENCE_ONLY_ALLOCATIONS_REQUIRED,
                ),
                arrayed,
                rows_of_32: support.ConfigurationFlags.contains(
                    D3D12_VIDEO_DECODE_CONFIGURATION_FLAG_HEIGHT_ALIGNMENT_MULTIPLE_32_REQUIRED,
                ),
                flags: support.ConfigurationFlags.0,
            });
        }
        Err(Error::Unavailable(String::from(
            "このプロファイルをデコードできません",
        )))
    }

    /// デコードの出力を渡すテクスチャへビデオプロセッサーで書けるか。問い合わせずに使うとエラー無しに何も書かないドライバーがある。
    pub fn converts(
        &self,
        configuration: D3D12_VIDEO_DECODE_CONFIGURATION,
        format: DXGI_FORMAT,
        width: u32,
        height: u32,
    ) -> (bool, String) {
        let colour = DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709;
        let mut support = D3D12_FEATURE_DATA_VIDEO_DECODE_CONVERSION_SUPPORT {
            NodeIndex: 0,
            Configuration: configuration,
            DecodeSample: D3D12_VIDEO_SAMPLE {
                Width: width,
                Height: height,
                Format: D3D12_VIDEO_FORMAT {
                    Format: format,
                    ColorSpace: colour,
                },
            },
            OutputFormat: D3D12_VIDEO_FORMAT {
                Format: format,
                ColorSpace: colour,
            },
            FrameRate: DXGI_RATIONAL {
                Numerator: 0,
                Denominator: 1,
            },
            BitRate: 0,
            ..Default::default()
        };
        let Ok(video) = self.video() else {
            return (false, no_video().to_string());
        };
        // SAFETY: a live local of exactly the size given.
        let asked = unsafe {
            video.CheckFeatureSupport(
                D3D12_FEATURE_VIDEO_DECODE_CONVERSION_SUPPORT,
                std::ptr::from_mut(&mut support).cast(),
                size_of::<D3D12_FEATURE_DATA_VIDEO_DECODE_CONVERSION_SUPPORT>() as u32,
            )
        };
        match asked {
            Err(error) => (false, format!("問い合わせに失敗（{error}）")),
            Ok(()) => {
                let on = support
                    .SupportFlags
                    .contains(D3D12_VIDEO_DECODE_CONVERSION_SUPPORT_FLAG_SUPPORTED);
                let range = support.ScaleSupport.OutputSizeRange;
                (
                    on,
                    format!(
                        "{} スケーリング {}〜{}x{}〜{}",
                        if on { "対応" } else { "**非対応**" },
                        range.MinWidth,
                        range.MaxWidth,
                        range.MinHeight,
                        range.MaxHeight,
                    ),
                )
            }
        }
    }

    pub fn decoder(
        &self,
        configuration: D3D12_VIDEO_DECODE_CONFIGURATION,
        format: DXGI_FORMAT,
        width: u32,
        height: u32,
        pictures: u32,
    ) -> Result<(ID3D12VideoDecoder, ID3D12VideoDecoderHeap), Error> {
        let description = D3D12_VIDEO_DECODER_DESC {
            NodeMask: 0,
            Configuration: configuration,
        };
        let heap = D3D12_VIDEO_DECODER_HEAP_DESC {
            NodeMask: 0,
            Configuration: configuration,
            DecodeWidth: width,
            DecodeHeight: height,
            Format: format,
            // 0 は「不明」（HEAP_DESC の Remarks）。60/1 だと先頭しか渡さないドライバーがある。
            FrameRate: DXGI_RATIONAL {
                Numerator: 0,
                Denominator: 1,
            },
            BitRate: 0,
            MaxDecodePictureBufferCount: pictures,
        };
        let video = self.video()?;
        // SAFETY: both descriptions are live locals; neither call keeps a pointer.
        unsafe {
            let decoder: ID3D12VideoDecoder = video
                .CreateVideoDecoder(&description)
                .map_err(|error| failed("CreateVideoDecoder", error))?;
            let heap: ID3D12VideoDecoderHeap = video
                .CreateVideoDecoderHeap(&heap)
                .map_err(|error| failed("CreateVideoDecoderHeap", error))?;
            Ok((decoder, heap))
        }
    }

    pub fn texture(
        &self,
        format: DXGI_FORMAT,
        width: u32,
        height: u32,
        reference_only: bool,
        layers: u32,
    ) -> Result<ID3D12Resource, Error> {
        texture(&self.raw, format, width, height, reference_only, layers)
    }

    pub fn upload(&self, size: u64) -> Result<ID3D12Resource, Error> {
        self.buffer(
            size,
            D3D12_HEAP_TYPE_UPLOAD,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )
    }

    pub(super) fn buffer(
        &self,
        size: u64,
        kind: D3D12_HEAP_TYPE,
        state: D3D12_RESOURCE_STATES,
    ) -> Result<ID3D12Resource, Error> {
        buffer(&self.raw, size, kind, D3D12_HEAP_FLAG_NONE, state)
    }

    /// バッファの先頭へ `data` を書く。バッファは `data` 以上のサイズであること。
    pub fn write(&self, buffer: &ID3D12Resource, data: &[u8]) -> Result<(), Error> {
        // 読み取り範囲を空にすると、ディスクリート GPU でキャッシュの無効化を省ける。
        Mapping::new(buffer, data.len(), 0..0)?.write(data);
        Ok(())
    }
}

impl Device {
    pub fn video(&self) -> Result<&ID3D12VideoDevice, Error> {
        self.video.as_ref().ok_or_else(no_video)
    }
}

fn no_video() -> Error {
    Error::Unavailable(String::from(
        "この GPU のドライバーは D3D12 のビデオデコードに対応していません",
    ))
}

/// プールのサーフェスを一枚。まず `ALLOW_SIMULTANEOUS_ACCESS` で（使い終えると自動で `COMMON` へ
/// 戻るので wgpu へ渡せる）、拒否されたら無しで。`layers` が 2 以上なら配列（参照専用だけ）。
pub(crate) fn texture(
    raw: &ID3D12Device,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
    reference_only: bool,
    layers: u32,
) -> Result<ID3D12Resource, Error> {
    let properties = heap(D3D12_HEAP_TYPE_DEFAULT);
    let mut description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: u64::from(width),
        Height: height,
        DepthOrArraySize: layers.max(1) as u16,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
    };
    let candidates: &[D3D12_RESOURCE_FLAGS] = if reference_only {
        &[D3D12_RESOURCE_FLAG_VIDEO_DECODE_REFERENCE_ONLY
            | D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE]
    } else {
        &[
            D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
            D3D12_RESOURCE_FLAG_NONE,
        ]
    };
    for flags in candidates.iter().copied() {
        description.Flags = flags;
        let mut texture: Option<ID3D12Resource> = None;
        // SAFETY: both descriptions are live locals; `texture` is written at most once.
        let made = unsafe {
            raw.CreateCommittedResource(
                &properties,
                D3D12_HEAP_FLAG_NONE,
                &description,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut texture,
            )
        };
        if made.is_ok()
            && let Some(texture) = texture
        {
            return Ok(texture);
        }
    }
    Err(Error::Unavailable(String::from(
        "サーフェスが確保できません",
    )))
}
