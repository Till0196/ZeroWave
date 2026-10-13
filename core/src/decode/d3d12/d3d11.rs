// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! D3D11 Video Decode（D3D11VA）。D3D12 のデコードが使えない GPU 向けのフォールバック。
//! 配列のスライスへデコードし、スロットごとの共有テクスチャへコピーして共有フェンスを進める
//! （`BIND_DECODER` のリソースを共有できるかはドライバー次第なのでコピーする）。

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_FENCE_FLAG_SHARED, ID3D12Device, ID3D12Fence, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_P010, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIResource1};
use windows::Win32::Media::MediaFoundation::{
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS,
    D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL, D3D12_VIDEO_DECODE_FRAME_ARGUMENT,
    D3D12_VIDEO_DECODE_PROFILE_H264,
};
use windows::core::{GUID, Interface};

use crate::decode::Error;
use crate::decode::d3d12::dx::{Owned, adapter_by_luid, failed, luid};

/// ビットストリームバッファのアラインメント（DXVA の H.264・HEVC が 128 バイトの倍数を求める）。
const ALIGNMENT: usize = 128;

const PENDING_TRIES: u32 = 100;

/// DXVA の `ConfigBitstreamRaw`：MPEG-2 は 1、H.264 は Short 形式の 2、HEVC は Short 形式しか無く 1。
fn raw_for(profile: GUID) -> u32 {
    if profile == D3D12_VIDEO_DECODE_PROFILE_H264 {
        2
    } else {
        1
    }
}

fn device_on(raw: &ID3D12Device) -> Result<(ID3D11Device, ID3D11DeviceContext), Error> {
    let adapter: IDXGIAdapter = adapter_by_luid(luid(raw))?;
    let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device = None;
    let mut context = None;
    // SAFETY: every out parameter is a live local written at most once.
    unsafe {
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|error| failed("D3D11CreateDevice", error))?;
    let (Some(device), Some(context)) = (device, context) else {
        return Err(Error::Unavailable(String::from(
            "D3D11 のデバイスがありません",
        )));
    };
    if let Ok(threads) = device.cast::<ID3D11Multithread>() {
        // SAFETY: the interface is alive for the length of the call.
        let _ = unsafe { threads.SetMultithreadProtected(true) };
    }
    Ok((device, context))
}

fn config(
    video: &ID3D11VideoDevice,
    profile: GUID,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
) -> Result<D3D11_VIDEO_DECODER_CONFIG, Error> {
    // SAFETY: the device is alive and every pointer is to a live local.
    unsafe {
        let count = video.GetVideoDecoderProfileCount();
        let listed = (0..count).any(|at| video.GetVideoDecoderProfile(at).ok() == Some(profile));
        if !listed {
            return Err(Error::Unavailable(format!(
                "D3D11 はこのプロファイルに対応していません（{}）",
                crate::decode::d3d12::dx::profile_name(profile)
            )));
        }
        if !video
            .CheckVideoDecoderFormat(&profile, format)
            .map_err(|error| failed("CheckVideoDecoderFormat", error))?
            .as_bool()
        {
            return Err(Error::Unavailable(String::from(
                "D3D11 はこの形式へデコードできません",
            )));
        }
        let description = D3D11_VIDEO_DECODER_DESC {
            Guid: profile,
            SampleWidth: width,
            SampleHeight: height,
            OutputFormat: format,
        };
        let configs = video
            .GetVideoDecoderConfigCount(&description)
            .map_err(|error| failed("GetVideoDecoderConfigCount", error))?;
        let want = raw_for(profile);
        for at in 0..configs {
            let mut config = D3D11_VIDEO_DECODER_CONFIG::default();
            if video
                .GetVideoDecoderConfig(&description, at, &mut config)
                .is_ok()
                && config.ConfigBitstreamRaw == want
            {
                return Ok(config);
            }
        }
        Err(Error::Unavailable(format!(
            "D3D11 に ConfigBitstreamRaw {want} の設定がありません"
        )))
    }
}

pub fn supports(
    raw: &ID3D12Device,
    profile: GUID,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
) -> Result<(), Error> {
    let (device, _) = device_on(raw)?;
    let video: ID3D11VideoDevice = device
        .cast()
        .map_err(|error| failed("ID3D11VideoDevice", error))?;
    config(&video, profile, format, width, height).map(|_| ())
}

pub struct Video {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    signaller: ID3D11DeviceContext4,
    video: ID3D11VideoContext,
    decoder: ID3D11VideoDecoder,
    /// デコード先。スライス番号がスロット番号で、DXVA の番号でもある。
    array: ID3D11Texture2D,
    views: Vec<ID3D11VideoDecoderOutputView>,
    shared: Vec<ID3D11Texture2D>,
    fence11: ID3D11Fence,
    fence12: ID3D12Fence,
    event: Owned,
    value: u64,
}

impl std::fmt::Debug for Video {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "D3D11Video({} スロット)", self.shared.len())
    }
}

// SAFETY: used from the decoding thread alone, and the device is multithread-protected.
unsafe impl Send for Video {}

impl Video {
    pub fn open(
        raw: &ID3D12Device,
        profile: GUID,
        format: DXGI_FORMAT,
        width: u32,
        height: u32,
        count: usize,
    ) -> Result<(Self, Vec<ID3D12Resource>), Error> {
        let (device, context) = device_on(raw)?;
        let video: ID3D11VideoDevice = device
            .cast()
            .map_err(|error| failed("ID3D11VideoDevice", error))?;
        let config = config(&video, profile, format, width, height)?;
        if count < usize::from(config.ConfigMinRenderTargetBuffCount) {
            return Err(Error::Unavailable(format!(
                "D3D11 のデコーダーがサーフェスを {} 枚求めます（{count} 枚しかありません）",
                config.ConfigMinRenderTargetBuffCount
            )));
        }
        // NV12 を共有できるか。P010 は問い合わせる API が無いので作って確かめる。
        let mut options = D3D11_FEATURE_DATA_D3D11_OPTIONS4::default();
        // SAFETY: the pointer is to a live local of exactly the size given.
        let asked = unsafe {
            device.CheckFeatureSupport(
                D3D11_FEATURE_D3D11_OPTIONS4,
                std::ptr::from_mut(&mut options).cast(),
                size_of::<D3D11_FEATURE_DATA_D3D11_OPTIONS4>() as u32,
            )
        };
        if format != DXGI_FORMAT_P010
            && (asked.is_err() || !options.ExtendedNV12SharedTextureSupported.as_bool())
        {
            return Err(Error::Unavailable(String::from(
                "D3D11 は NV12 のテクスチャを共有できません",
            )));
        }
        let description = D3D11_VIDEO_DECODER_DESC {
            Guid: profile,
            SampleWidth: width,
            SampleHeight: height,
            OutputFormat: format,
        };
        // SAFETY: both descriptions are live locals.
        let decoder = unsafe { video.CreateVideoDecoder(&description, &config) }
            .map_err(|error| failed("CreateVideoDecoder", error))?;

        let texture = |array: u32, bind: D3D11_BIND_FLAG, misc: u32| {
            let description = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: array,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: bind.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: misc,
            };
            let mut made = None;
            // SAFETY: the description is a live local and `made` is written at most once.
            unsafe { device.CreateTexture2D(&description, None, Some(&mut made)) }
                .map_err(|error| failed("CreateTexture2D", error))?;
            made.ok_or_else(|| Error::Unavailable(String::from("テクスチャが作れません")))
        };
        let array = texture(count as u32, D3D11_BIND_DECODER, 0)?;
        let mut views = Vec::with_capacity(count);
        for slice in 0..count {
            let description = D3D11_VIDEO_DECODER_OUTPUT_VIEW_DESC {
                DecodeProfile: profile,
                ViewDimension: D3D11_VDOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_DECODER_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VDOV {
                        ArraySlice: slice as u32,
                    },
                },
            };
            let mut view = None;
            // SAFETY: the array and the description are alive for the call.
            unsafe { video.CreateVideoDecoderOutputView(&array, &description, Some(&mut view)) }
                .map_err(|error| failed("CreateVideoDecoderOutputView", error))?;
            views.push(view.ok_or_else(|| {
                Error::Unavailable(String::from("デコード先のビューが作れません"))
            })?);
        }

        let mut shared = Vec::with_capacity(count);
        let mut opened = Vec::with_capacity(count);
        for _ in 0..count {
            let texture = texture(
                1,
                D3D11_BIND_SHADER_RESOURCE,
                (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
            )?;
            let resource: IDXGIResource1 = texture
                .cast()
                .map_err(|error| failed("IDXGIResource1", error))?;
            let handle = Owned::shared_dxgi(&resource)?;
            let mut resource12: Option<ID3D12Resource> = None;
            // SAFETY: 共有テクスチャの有効な NT ハンドル。
            let result = unsafe { raw.OpenSharedHandle(handle.raw(), &mut resource12) };
            drop(handle);
            result.map_err(|error| failed("OpenSharedHandle", error))?;
            let resource12 = resource12
                .ok_or_else(|| Error::Unavailable(String::from("共有テクスチャが開けません")))?;
            shared.push(texture);
            opened.push(resource12);
        }
        // SAFETY: the device is alive.
        let fence12: ID3D12Fence = unsafe { raw.CreateFence(0, D3D12_FENCE_FLAG_SHARED) }
            .map_err(|error| failed("CreateFence", error))?;
        let handle = Owned::shared(raw, &fence12)?;
        let mut fence11: Option<ID3D11Fence> = None;
        let result = match device.cast::<ID3D11Device5>() {
            // SAFETY: 共有したフェンスの有効な NT ハンドル。
            Ok(device5) => unsafe { device5.OpenSharedFence(handle.raw(), &mut fence11) }
                .map_err(|error| failed("OpenSharedFence", error)),
            Err(error) => Err(failed("ID3D11Device5", error)),
        };
        drop(handle);
        result?;
        let fence11 =
            fence11.ok_or_else(|| Error::Unavailable(String::from("フェンスが開けません")))?;
        let signaller: ID3D11DeviceContext4 = context
            .cast()
            .map_err(|error| failed("ID3D11DeviceContext4", error))?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(|error| failed("ID3D11VideoContext", error))?;
        let event = Owned::event()?;

        Ok((
            Self {
                _device: device,
                context,
                signaller,
                video: video_context,
                decoder,
                array,
                views,
                shared,
                fence11,
                fence12,
                event,
                value: 0,
            },
            opened,
        ))
    }

    /// 一枚を提出し、スロットの共有テクスチャへコピーして、書き終わると到達するフェンスの値を返す。
    /// `macroblocks` は MPEG-2 と H.264 でビットストリームとスライス情報のバッファに添える。
    pub fn submit(
        &mut self,
        slot: usize,
        bytes: &[u8],
        arguments: &[D3D12_VIDEO_DECODE_FRAME_ARGUMENT],
        macroblocks: u32,
    ) -> Result<u64, Error> {
        let view = self
            .views
            .get(slot)
            .ok_or(Error::Damaged("a slot past the decoder's views"))?
            .clone();
        let mut tries = 0;
        loop {
            // SAFETY: the decoder and the view are this object's own.
            match unsafe { self.video.DecoderBeginFrame(&self.decoder, &view, 0, None) } {
                Ok(()) => break,
                Err(error) if error.code().0 == 0x8000_000Au32 as i32 && tries < PENDING_TRIES => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(error) => return Err(failed("DecoderBeginFrame", error)),
            }
        }
        let result = self.fill_and_submit(bytes, arguments, macroblocks);
        // SAFETY: a frame was begun on this decoder a moment ago.
        let ended = unsafe { self.video.DecoderEndFrame(&self.decoder) };
        result?;
        ended.map_err(|error| failed("DecoderEndFrame", error))?;

        // NV12 と P010 は D3D11 では二つのプレーンを一つのサブリソースとしてコピーする。
        let shared = self
            .shared
            .get(slot)
            .ok_or(Error::Damaged("a slot past the shared textures"))?;
        self.value += 1;
        // SAFETY: both textures are ours, same format and size; the fence is shared with D3D12.
        unsafe {
            self.context
                .CopySubresourceRegion(shared, 0, 0, 0, 0, &self.array, slot as u32, None);
            self.signaller
                .Signal(&self.fence11, self.value)
                .map_err(|error| failed("Signal", error))?;
            self.context.Flush();
        }
        Ok(self.value)
    }

    fn fill_and_submit(
        &mut self,
        bytes: &[u8],
        arguments: &[D3D12_VIDEO_DECODE_FRAME_ARGUMENT],
        macroblocks: u32,
    ) -> Result<(), Error> {
        let mut descriptions = Vec::with_capacity(arguments.len() + 1);
        for argument in arguments {
            let kind = match argument.Type {
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_PICTURE_PARAMETERS => {
                    D3D11_VIDEO_DECODER_BUFFER_PICTURE_PARAMETERS
                }
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_INVERSE_QUANTIZATION_MATRIX => {
                    D3D11_VIDEO_DECODER_BUFFER_INVERSE_QUANTIZATION_MATRIX
                }
                D3D12_VIDEO_DECODE_ARGUMENT_TYPE_SLICE_CONTROL => {
                    D3D11_VIDEO_DECODER_BUFFER_SLICE_CONTROL
                }
                _ => continue,
            };
            // SAFETY: the front half holds `Size` bytes at `pData` until this call returns.
            let data = unsafe {
                std::slice::from_raw_parts(argument.pData.cast::<u8>(), argument.Size as usize)
            };
            self.put(kind, data, data.len())?;
            descriptions.push(D3D11_VIDEO_DECODER_BUFFER_DESC {
                BufferType: kind,
                DataSize: data.len() as u32,
                NumMBsInBuffer: if kind == D3D11_VIDEO_DECODER_BUFFER_SLICE_CONTROL {
                    macroblocks
                } else {
                    0
                },
                ..Default::default()
            });
        }
        let padded = bytes.len().next_multiple_of(ALIGNMENT);
        let written = self.put(D3D11_VIDEO_DECODER_BUFFER_BITSTREAM, bytes, padded)?;
        descriptions.push(D3D11_VIDEO_DECODER_BUFFER_DESC {
            BufferType: D3D11_VIDEO_DECODER_BUFFER_BITSTREAM,
            DataSize: written as u32,
            NumMBsInBuffer: macroblocks,
            ..Default::default()
        });
        // SAFETY: every buffer named was filled and released above.
        unsafe {
            self.video
                .SubmitDecoderBuffers(&self.decoder, &descriptions)
        }
        .map_err(|error| failed("SubmitDecoderBuffers", error))
    }

    /// ドライバーのバッファへ `data` をコピーし、`size` まで 0 で埋める。実際に書いたサイズを返す。
    fn put(
        &mut self,
        kind: D3D11_VIDEO_DECODER_BUFFER_TYPE,
        data: &[u8],
        size: usize,
    ) -> Result<usize, Error> {
        let mut capacity = 0u32;
        let mut pointer = std::ptr::null_mut();
        // SAFETY: both out parameters are live locals.
        unsafe {
            self.video
                .GetDecoderBuffer(&self.decoder, kind, &mut capacity, &mut pointer)
        }
        .map_err(|error| failed("GetDecoderBuffer", error))?;
        let capacity = capacity as usize;
        let fits = data.len() <= capacity && !pointer.is_null();
        if fits {
            let size = size.min(capacity);
            // SAFETY: the driver's buffer is `capacity` bytes and writable until released.
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), pointer.cast::<u8>(), data.len());
                std::ptr::write_bytes(pointer.cast::<u8>().add(data.len()), 0, size - data.len());
            }
        }
        // SAFETY: the buffer was taken a moment ago and is not touched again.
        unsafe { self.video.ReleaseDecoderBuffer(&self.decoder, kind) }
            .map_err(|error| failed("ReleaseDecoderBuffer", error))?;
        if !fits {
            return Err(Error::Unavailable(format!(
                "D3D11 のバッファに入りません（{} バイト、バッファは {capacity}）",
                data.len()
            )));
        }
        Ok(size.min(capacity))
    }

    pub fn wait(&self, value: u64) -> Result<(), Error> {
        self.event.wait_fence(&self.fence12, value)
    }

    pub fn wait_idle(&self) {
        if let Err(error) = self.wait(self.value) {
            tracing::warn!("D3D11VA: 待機に失敗: {error:?}");
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.wait_idle();
    }
}
