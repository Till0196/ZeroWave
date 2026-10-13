// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! VideoToolbox・CoreFoundation・CoreMedia・CoreVideo を自前で宣言した `extern "C"` で呼ぶ。
//! `kind = "framework"` は Apple 以外でエラーなので `cfg_attr` で付ける。
//! Create・Copy が返した物は所有（[`Owned`] で一度だけ `CFRelease`）、Get が返した物は借用。

#![allow(non_upper_case_globals, non_snake_case)]

use std::ffi::{c_int, c_void};

pub type OSStatus = i32;
pub type OSType = u32;
pub type Boolean = u8;
pub type CFIndex = isize;
pub type CFTypeRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFNumberType = CFIndex;
pub type CMFormatDescriptionRef = *mut c_void;
pub type CMBlockBufferRef = *mut c_void;

/// `kCMBlockBufferAssureMemoryNowFlag`。作る時点で本当に確保させる。
pub const ASSURE_MEMORY_NOW: u32 = 1;

/// `kVTDecodeFrame_EnableAsynchronousDecompression`。立てないと `DecodeFrame` は同期になる。
pub const DECODE_ASYNCHRONOUSLY: u32 = 1;
pub type CMSampleBufferRef = *mut c_void;
pub type CVPixelBufferRef = *mut c_void;
pub type CVImageBufferRef = *mut c_void;
pub type VTDecompressionSessionRef = *mut c_void;
pub type CVReturn = i32;
pub type CVMetalTextureCacheRef = *mut c_void;
pub type CVMetalTextureRef = *mut c_void;
pub type MetalObject = *mut c_void;
pub type MTLPixelFormat = usize;

pub const kCFAllocatorDefault: CFAllocatorRef = std::ptr::null();

pub const kCFNumberSInt32Type: CFNumberType = 3;

pub const MTLPixelFormatR8Unorm: MTLPixelFormat = 10;
pub const MTLPixelFormatR16Unorm: MTLPixelFormat = 20;
pub const MTLPixelFormatRG8Unorm: MTLPixelFormat = 30;
pub const MTLPixelFormatRG16Unorm: MTLPixelFormat = 60;
pub const MTLPixelFormatRGBA16Float: MTLPixelFormat = 115;

/// `'RGhA'`：f16 の RGBA。フレーム補間はこれだけ受ける。
pub const kCVPixelFormatType_64RGBAHalf: OSType = 0x5247_6841;

pub const kCVPixelBufferLock_ReadOnly: u64 = 0x0000_0001;

pub const kCMVideoCodecType_H264: OSType = 0x6176_6331;
pub const kCMVideoCodecType_HEVC: OSType = 0x6876_6331;
pub const kCMVideoCodecType_MPEG2Video: OSType = 0x6d70_3276;

/// `'420v'`。ビデオレンジでないと値域が伸び、ソフトウェアデコーダーと合わなくなる。
pub const kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: OSType = 0x3432_3076;

pub const kCVPixelFormatType_420YpCbCr10BiPlanarVideoRange: OSType = 0x7834_3230;

pub const kVTPropertyNotSupportedErr: OSStatus = -12900;
pub const kVTCouldNotFindVideoDecoderErr: OSStatus = -12906;
pub const kVTVideoDecoderBadDataErr: OSStatus = -12909;
pub const kVTVideoDecoderUnsupportedDataFormatErr: OSStatus = -12910;
pub const kVTVideoDecoderMalfunctionErr: OSStatus = -12911;
pub const kVTVideoDecoderNotAvailableNowErr: OSStatus = -12913;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VTDecompressionOutputCallbackRecord {
    pub callback: Option<VTDecompressionOutputCallback>,
    pub refcon: *mut c_void,
}

pub type VTDecompressionOutputCallback = unsafe extern "C" fn(
    refcon: *mut c_void,
    source_frame_refcon: *mut c_void,
    status: OSStatus,
    info: u32,
    image: CVImageBufferRef,
    presentation: CMTime,
    duration: CMTime,
);

#[repr(C)]
#[derive(Debug)]
pub struct Opaque([u8; 0]);

#[cfg_attr(
    target_vendor = "apple",
    link(name = "CoreFoundation", kind = "framework")
)]
unsafe extern "C" {
    pub fn CFRelease(value: CFTypeRef);
    pub fn CFRetain(value: CFTypeRef) -> CFTypeRef;
    pub fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: CFIndex,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> CFDictionaryRef;
    pub fn CFNumberCreate(
        allocator: CFAllocatorRef,
        kind: CFNumberType,
        value: *const c_void,
    ) -> CFNumberRef;
    pub fn CFBooleanGetValue(value: CFTypeRef) -> Boolean;
    pub static kCFTypeDictionaryKeyCallBacks: Opaque;
    pub static kCFTypeDictionaryValueCallBacks: Opaque;
    pub static kCFBooleanTrue: CFTypeRef;
    pub static kCFBooleanFalse: CFTypeRef;
    /// 「解放しないでほしい」アロケーター。
    pub static kCFAllocatorNull: CFAllocatorRef;
}

#[cfg_attr(target_vendor = "apple", link(name = "CoreVideo", kind = "framework"))]
unsafe extern "C" {
    pub fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> CVReturn;
    pub fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> CVReturn;
    pub fn CVPixelBufferGetBaseAddressOfPlane(
        buffer: CVPixelBufferRef,
        plane: usize,
    ) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRowOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
    pub fn CVPixelBufferGetWidthOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
    pub fn CVPixelBufferGetHeightOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
    pub fn CVPixelBufferGetPlaneCount(buffer: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetPixelFormatType(buffer: CVPixelBufferRef) -> OSType;
    pub fn CVPixelBufferGetWidth(buffer: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetHeight(buffer: CVPixelBufferRef) -> usize;
    pub static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    pub static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
    pub static kCVPixelBufferMetalCompatibilityKey: CFStringRef;
    pub fn CVPixelBufferCreate(
        allocator: CFAllocatorRef,
        width: usize,
        height: usize,
        format: OSType,
        attributes: CFDictionaryRef,
        out: *mut CVPixelBufferRef,
    ) -> CVReturn;
    pub static kCVMetalTextureUsage: CFStringRef;
    pub fn CVMetalTextureCacheCreate(
        allocator: CFAllocatorRef,
        cache_attributes: CFDictionaryRef,
        device: MetalObject,
        texture_attributes: CFDictionaryRef,
        out: *mut CVMetalTextureCacheRef,
    ) -> CVReturn;
    pub fn CVMetalTextureCacheCreateTextureFromImage(
        allocator: CFAllocatorRef,
        cache: CVMetalTextureCacheRef,
        source: CVImageBufferRef,
        texture_attributes: CFDictionaryRef,
        format: MTLPixelFormat,
        width: usize,
        height: usize,
        plane: usize,
        out: *mut CVMetalTextureRef,
    ) -> CVReturn;
    /// Get なので借用。`CVMetalTexture` が生きている間だけ有効。
    pub fn CVMetalTextureGetTexture(texture: CVMetalTextureRef) -> MetalObject;
    pub fn CVMetalTextureCacheFlush(cache: CVMetalTextureCacheRef, options: u64);
}

#[cfg_attr(target_vendor = "apple", link(name = "CoreMedia", kind = "framework"))]
unsafe extern "C" {
    pub fn CMVideoFormatDescriptionCreate(
        allocator: CFAllocatorRef,
        codec: OSType,
        width: i32,
        height: i32,
        extensions: CFDictionaryRef,
        out: *mut CMFormatDescriptionRef,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CFAllocatorRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_unit_header_length: c_int,
        out: *mut CMFormatDescriptionRef,
    ) -> OSStatus;
    pub fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: CFAllocatorRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_unit_header_length: c_int,
        extensions: CFDictionaryRef,
        out: *mut CMFormatDescriptionRef,
    ) -> OSStatus;
    pub fn CMBlockBufferCreateWithMemoryBlock(
        structure_allocator: CFAllocatorRef,
        memory: *mut c_void,
        length: usize,
        block_allocator: CFAllocatorRef,
        source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        out: *mut CMBlockBufferRef,
    ) -> OSStatus;
    /// 確保済みのブロックへ中身をコピーする（借用したメモリだとデコード中に消えうる）。
    pub fn CMBlockBufferReplaceDataBytes(
        source: *const c_void,
        into: CMBlockBufferRef,
        offset: usize,
        length: usize,
    ) -> OSStatus;
    pub fn CMSampleBufferCreate(
        allocator: CFAllocatorRef,
        data: CMBlockBufferRef,
        data_ready: Boolean,
        make_ready: *const c_void,
        make_ready_refcon: *mut c_void,
        format: CMFormatDescriptionRef,
        samples: CFIndex,
        timings: CFIndex,
        timing_array: *const c_void,
        sizes: CFIndex,
        size_array: *const usize,
        out: *mut CMSampleBufferRef,
    ) -> OSStatus;
}

#[cfg_attr(
    target_vendor = "apple",
    link(name = "VideoToolbox", kind = "framework")
)]
unsafe extern "C" {
    pub fn VTDecompressionSessionCreate(
        allocator: CFAllocatorRef,
        format: CMFormatDescriptionRef,
        specification: CFDictionaryRef,
        attributes: CFDictionaryRef,
        callback: *const VTDecompressionOutputCallbackRecord,
        out: *mut VTDecompressionSessionRef,
    ) -> OSStatus;
    pub fn VTDecompressionSessionDecodeFrame(
        session: VTDecompressionSessionRef,
        sample: CMSampleBufferRef,
        flags: u32,
        source_frame_refcon: *mut c_void,
        info_out: *mut u32,
    ) -> OSStatus;
    pub fn VTDecompressionSessionWaitForAsynchronousFrames(
        session: VTDecompressionSessionRef,
    ) -> OSStatus;
    pub fn VTDecompressionSessionFinishDelayedFrames(
        session: VTDecompressionSessionRef,
    ) -> OSStatus;
    pub fn VTDecompressionSessionInvalidate(session: VTDecompressionSessionRef);
    pub fn VTDecompressionSessionCanAcceptFormatDescription(
        session: VTDecompressionSessionRef,
        format: CMFormatDescriptionRef,
    ) -> Boolean;
    pub fn VTSessionSetProperty(
        session: *mut c_void,
        key: CFStringRef,
        value: CFTypeRef,
    ) -> OSStatus;
    pub fn VTSessionCopyProperty(
        session: *mut c_void,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    /// 実際にハードウェアデコーダーが使われたか。MPEG-2 で false は想定内。
    pub static kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder: CFStringRef;
    pub static kVTDecompressionPropertyKey_OutputPoolRequestedMinimumBufferCount: CFStringRef;
    /// 偽にすると VideoToolbox 内のソフトウェアデコーダーだけ（[`crate::decode::Prefer::System`]）。
    pub static kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: CFStringRef;
}

/// 所有している CoreFoundation の値。Create か Copy が返したものだけを入れる。
#[derive(Debug)]
pub struct Owned<T>(pub *mut T);

impl<T> Owned<T> {
    /// # Safety
    /// `raw` は Create か Copy が返した未解放の値で、所有権をここへ渡しきること。
    pub unsafe fn new(raw: *mut T) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    pub fn as_ptr(&self) -> *mut T {
        self.0
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        // SAFETY: 所有している値をちょうど一度手放す。
        unsafe { CFRelease(self.0.cast()) };
    }
}

// SAFETY: 参照数の増減はスレッド安全で、値は一つのデコーダーだけが持つ（`Sync` は求めない）。
unsafe impl<T> Send for Owned<T> {}

/// Create・Copy を呼び、成功したら書かれた値を所有する。失敗なら状態の値を返す。
/// # Safety
/// `create` は渡した出力へ Create か Copy の結果を書く呼び出しであること。
pub unsafe fn created<T>(
    create: impl FnOnce(*mut *mut T) -> OSStatus,
) -> Result<Option<Owned<T>>, OSStatus> {
    let mut raw: *mut T = std::ptr::null_mut();
    let status = create(&raw mut raw);
    if status != 0 {
        return Err(status);
    }
    // SAFETY: 呼び出し側が保証するとおり、成功した Create・Copy の結果。
    Ok(unsafe { Owned::new(raw) })
}

/// 所有している CVPixelBuffer。
#[derive(Debug)]
pub struct PixelBuffer(Owned<c_void>);

impl PixelBuffer {
    /// # Safety
    /// `owned` は CVPixelBuffer であること。
    pub unsafe fn new(owned: Owned<c_void>) -> Self {
        Self(owned)
    }

    /// 借用で、受け取った側が解放してはならない。
    pub fn as_ptr(&self) -> CVPixelBufferRef {
        self.0.as_ptr()
    }

    pub fn format(&self) -> OSType {
        // SAFETY: 生きたピクセルバッファから読むだけ。
        unsafe { CVPixelBufferGetPixelFormatType(self.as_ptr()) }
    }

    pub fn width(&self) -> usize {
        // SAFETY: 同上。
        unsafe { CVPixelBufferGetWidth(self.as_ptr()) }
    }

    pub fn height(&self) -> usize {
        // SAFETY: 同上。
        unsafe { CVPixelBufferGetHeight(self.as_ptr()) }
    }

    /// 読み取り専用のロックを掛ける。掛からなければ `None`。
    pub fn lock_read(&self) -> Option<ReadLock<'_>> {
        // SAFETY: 生きたピクセルバッファ。ロックを外すのは `ReadLock` の `Drop`。
        let locked =
            unsafe { CVPixelBufferLockBaseAddress(self.as_ptr(), kCVPixelBufferLock_ReadOnly) };
        (locked == 0).then_some(ReadLock(self))
    }
}

/// 読み取り専用のロック。生きている間だけピクセルデータのアドレスが有効。
#[derive(Debug)]
pub struct ReadLock<'a>(&'a PixelBuffer);

impl ReadLock<'_> {
    pub fn planes(&self) -> usize {
        // SAFETY: ロックの掛かった生きたピクセルバッファ。
        unsafe { CVPixelBufferGetPlaneCount(self.0.as_ptr()) }
    }

    /// プレーン `plane` の中身と、一行のバイト数と行数。空なら `None`。
    pub fn plane(&self, plane: usize) -> Option<(&[u8], usize, usize)> {
        if plane >= self.planes() {
            return None;
        }
        let buffer = self.0.as_ptr();
        // SAFETY: `plane` はプレーンの数より小さい。アドレスはロックの内側で有効。
        let (base, pitch, rows) = unsafe {
            (
                CVPixelBufferGetBaseAddressOfPlane(buffer, plane),
                CVPixelBufferGetBytesPerRowOfPlane(buffer, plane),
                CVPixelBufferGetHeightOfPlane(buffer, plane),
            )
        };
        if base.is_null() || pitch == 0 || rows == 0 {
            return None;
        }
        // SAFETY: `base` から `pitch * rows` バイトがそのプレーンで、ロックより長くは借りない。
        let bytes = unsafe { std::slice::from_raw_parts(base.cast::<u8>(), pitch * rows) };
        Some((bytes, pitch, rows))
    }
}

impl Drop for ReadLock<'_> {
    fn drop(&mut self) {
        // SAFETY: 掛けたロックを同じフラグで外す。
        unsafe { CVPixelBufferUnlockBaseAddress(self.0.as_ptr(), kCVPixelBufferLock_ReadOnly) };
    }
}

pub fn status(what: &str, code: OSStatus) -> String {
    let name = match code {
        0 => "noErr",
        kVTPropertyNotSupportedErr => "kVTPropertyNotSupportedErr",
        -12902 => "kVTParameterErr",
        -12903 => "kVTInvalidSessionErr",
        -12904 => "kVTAllocationFailedErr",
        kVTCouldNotFindVideoDecoderErr => "kVTCouldNotFindVideoDecoderErr",
        -12907 => "kVTCouldNotCreateInstanceErr",
        kVTVideoDecoderBadDataErr => "kVTVideoDecoderBadDataErr",
        kVTVideoDecoderUnsupportedDataFormatErr => "kVTVideoDecoderUnsupportedDataFormatErr",
        kVTVideoDecoderMalfunctionErr => "kVTVideoDecoderMalfunctionErr",
        kVTVideoDecoderNotAvailableNowErr => "kVTVideoDecoderNotAvailableNowErr",
        -12916 => "kVTFormatDescriptionChangeNotSupportedErr",
        -17690 => "kVTVideoDecoderRemovedErr",
        -17691 => "kVTSessionMalfunctionErr",
        -17694 => "kVTVideoDecoderReferenceMissingErr",
        -17696 => "kVTVideoDecoderUnknownErr",
        -8969 => "codecBadDataErr",
        -8970 => "codecExtensionNotFoundErr",
        _ => "",
    };
    if name.is_empty() {
        format!("{what}: OSStatus {code}")
    } else {
        format!("{what}: {name} ({code})")
    }
}

/// CFDictionary を一つ組み立てる。CFDictionary がキーと値を保持するので、作った値はここで手放してよい。
/// # Safety
/// `pairs` のキーと値はいずれも生きた CoreFoundation の値であること。
pub unsafe fn dictionary(pairs: &[(CFStringRef, CFTypeRef)]) -> Option<Owned<c_void>> {
    let keys: Vec<*const c_void> = pairs.iter().map(|(key, _)| *key).collect();
    let values: Vec<*const c_void> = pairs.iter().map(|(_, value)| *value).collect();
    // SAFETY: 二つの配列は同じ長さで、その長さを渡している。
    let raw = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as CFIndex,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    };
    // SAFETY: Create なので所有物。
    unsafe { Owned::new(raw.cast_mut()) }
}

pub fn number(value: i32) -> Option<Owned<c_void>> {
    // SAFETY: `value` は生きた 32 ビット整数のローカル変数。
    let raw = unsafe {
        CFNumberCreate(
            kCFAllocatorDefault,
            kCFNumberSInt32Type,
            std::ptr::from_ref(&value).cast(),
        )
    };
    // SAFETY: Create なので所有物。
    unsafe { Owned::new(raw.cast_mut()) }
}
