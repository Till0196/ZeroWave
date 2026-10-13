// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! oneVPL / Media SDK のバインディング（intel/libvpl `api/vpl/mfx*.h`、MIT）。
//! 構造体の大きさとフィールドのオフセットは下の `const` で確かめている。フィールドを足すときは測り直す。
//! 呼ぶのは 1.x の形。`MFXInitEx` が無いランタイムには 2.x の `MFXInitialize` で入る。

use std::ffi::c_void;

pub type Status = i32;
pub type Session = *mut c_void;
pub type SyncPoint = *mut c_void;

pub const ERR_NONE: Status = 0;
pub const ERR_MORE_DATA: Status = -10;
pub const ERR_MORE_SURFACE: Status = -11;
pub const ERR_INCOMPATIBLE_VIDEO_PARAM: Status = -14;
pub const WRN_DEVICE_BUSY: Status = 2;
pub const WRN_VIDEO_PARAM_CHANGED: Status = 3;

// `HARDWARE`〜`HARDWARE4` は Windows の `d3d12` のビルドだけ、`VIA_D3D11` は Windows だけで使う。
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const IMPL_HARDWARE: i32 = 2;
pub const IMPL_HARDWARE_ANY: i32 = 4;
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const IMPL_HARDWARE2: i32 = 5;
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const IMPL_HARDWARE3: i32 = 6;
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const IMPL_HARDWARE4: i32 = 7;
#[cfg_attr(not(windows), allow(dead_code))]
pub const IMPL_VIA_D3D11: i32 = 0x0300;
#[cfg_attr(windows, allow(dead_code))]
pub const IMPL_VIA_VAAPI: i32 = 0x0400;
pub const ACCEL_MODE_VIA_D3D11: u32 = 0x0300;
pub const ACCEL_MODE_VIA_VAAPI: u32 = 0x0400;

#[cfg_attr(windows, allow(dead_code))]
pub const HANDLE_VA_DISPLAY: i32 = 4;

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}
pub const CODEC_AVC: u32 = fourcc(b"AVC ");
pub const CODEC_HEVC: u32 = fourcc(b"HEVC");
pub const CODEC_MPEG2: u32 = fourcc(b"MPG2");
pub const FOURCC_NV12: u32 = fourcc(b"NV12");
pub const FOURCC_P010: u32 = fourcc(b"P010");

pub const CHROMAFORMAT_YUV420: u16 = 1;
pub const PICSTRUCT_UNKNOWN: u16 = 0x00;
pub const PICSTRUCT_PROGRESSIVE: u16 = 0x01;
pub const PICSTRUCT_FIELD_BFF: u16 = 0x04;
pub const IOPATTERN_IN_SYSTEM_MEMORY: u16 = 0x02;
pub const IOPATTERN_OUT_SYSTEM_MEMORY: u16 = 0x20;
// 以下は映像フィルターの Intel 実装（`output/video/host/vpl.rs`）だけが使う。
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const IOPATTERN_IN_VIDEO_MEMORY: u16 = 0x01;
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const IOPATTERN_OUT_VIDEO_MEMORY: u16 = 0x10;
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const HANDLE_D3D11_DEVICE: i32 = 3;
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const RESOURCE_DX11_TEXTURE: i32 = 5;
/// `mfxFrameSurfaceInterface` の関数の位置：`Context`（ポインター）と `Version`・`reserved1[3]`（8 バイト）の後に
/// `AddRef`・`Release`・`GetRefCounter`・`Map`・`Unmap`・`GetNativeHandle` と並ぶ。
const INTERFACE_TABLE: usize = size_of::<*mut c_void>() + 8;
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const INTERFACE_RELEASE: usize = INTERFACE_TABLE + size_of::<*mut c_void>();
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const INTERFACE_GET_NATIVE_HANDLE: usize = INTERFACE_TABLE + 5 * size_of::<*mut c_void>();
pub const BITSTREAM_COMPLETE_FRAME: u16 = 0x0001;

pub const EXTBUFF_VPP_DEINTERLACING: u32 = fourcc(b"VPDI");
pub const DEINTERLACING_BOB: u16 = 1;
pub const DEINTERLACING_ADVANCED: u16 = 2;

#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const EXTBUFF_VPP_AI_FRAME_INTERPOLATION: u32 = fourcc(b"VAFI");
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub const AI_FRAME_INTERPOLATION_MODE_DEFAULT: i32 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub struct ExtVppAiFrameInterpolation {
    pub header: ExtBuffer,
    pub mode: i32,
    pub enable_scd: u16,
    _pad: u16,
    _reserved1: [u32; 24],
    _reserved2: [*mut c_void; 8],
}

#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
impl ExtVppAiFrameInterpolation {
    pub fn new(mode: i32) -> Self {
        Self {
            header: ExtBuffer {
                buffer_id: EXTBUFF_VPP_AI_FRAME_INTERPOLATION,
                buffer_sz: size_of::<Self>() as u32,
            },
            mode,
            enable_scd: 1,
            _pad: 0,
            _reserved1: [0; 24],
            _reserved2: [std::ptr::null_mut(); 8],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
pub struct Variant {
    pub version: u16,
    pub kind: u32,
    pub data: u64,
}

#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
impl Variant {
    pub const TYPE_U32: u32 = 5;

    pub fn u32(value: u32) -> Self {
        Self {
            version: 0x0101,
            kind: Self::TYPE_U32,
            data: u64::from(value),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Version {
    pub minor: u16,
    pub major: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InitExt {
    pub ext_param: *mut *mut ExtBuffer,
    pub num_ext_param: u16,
}

/// ヘッダーと同じ共用体にする（大きさがポインターの幅で変わる）。
#[repr(C)]
#[derive(Clone, Copy)]
pub union InitExtOrReserved {
    pub ext: InitExt,
    _reserved2: [u16; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InitParam {
    pub implementation: i32,
    pub version: Version,
    pub external_threads: u16,
    pub ext: InitExtOrReserved,
    pub gpu_copy: u16,
    _reserved: [u16; 21],
}

impl std::fmt::Debug for InitParam {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("InitParam")
    }
}

impl InitParam {
    pub fn new(implementation: i32) -> Self {
        Self {
            implementation,
            version: Version { minor: 0, major: 1 },
            external_threads: 0,
            ext: InitExtOrReserved {
                ext: InitExt {
                    ext_param: std::ptr::null_mut(),
                    num_ext_param: 0,
                },
            },
            gpu_copy: 0,
            _reserved: [0; 21],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InitializationParam {
    pub acceleration_mode: u32,
    _reserved: [u16; 3],
    pub num_ext_param: u16,
    pub ext_param: *mut *mut ExtBuffer,
    pub vendor_impl_id: u32,
    _reserved2: [u32; 3],
}

impl InitializationParam {
    pub fn new(acceleration_mode: u32, vendor_impl_id: u32) -> Self {
        Self {
            acceleration_mode,
            _reserved: [0; 3],
            num_ext_param: 0,
            ext_param: std::ptr::null_mut(),
            vendor_impl_id,
            _reserved2: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExtBuffer {
    pub buffer_id: u32,
    pub buffer_sz: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExtVppDeinterlacing {
    pub header: ExtBuffer,
    pub mode: u16,
    pub telecine_pattern: u16,
    pub telecine_location: u16,
    _reserved: [u16; 9],
}

impl ExtVppDeinterlacing {
    pub fn new(mode: u16) -> Self {
        Self {
            header: ExtBuffer {
                buffer_id: EXTBUFF_VPP_DEINTERLACING,
                buffer_sz: size_of::<Self>() as u32,
            },
            mode,
            telecine_pattern: 0,
            telecine_location: 0,
            _reserved: [0; 9],
        }
    }
}

#[repr(C, packed(4))]
#[derive(Debug, Clone, Copy)]
pub struct FrameInfo {
    _reserved: [u32; 4],
    pub channel_id: u16,
    pub bit_depth_luma: u16,
    pub bit_depth_chroma: u16,
    /// 10 ビットのサンプルが 2 バイトの上位に詰めてあるか（P010 なら 1）。
    pub shift: u16,
    _frame_id: [u16; 4],
    pub fourcc: u32,
    pub width: u16,
    pub height: u16,
    pub crop_x: u16,
    pub crop_y: u16,
    pub crop_w: u16,
    pub crop_h: u16,
    pub frame_rate_ext_n: u32,
    pub frame_rate_ext_d: u32,
    _reserved3: u16,
    pub aspect_ratio_w: u16,
    pub aspect_ratio_h: u16,
    pub pic_struct: u16,
    pub chroma_format: u16,
    _reserved2: u16,
}

#[repr(C, packed(4))]
#[derive(Debug, Clone, Copy)]
pub struct InfoMfx {
    _reserved: [u32; 7],
    pub low_power: u16,
    pub brc_param_multiplier: u16,
    pub frame_info: FrameInfo,
    pub codec_id: u32,
    pub codec_profile: u16,
    pub codec_level: u16,
    _num_thread: u16,
    pub decoded_order: u16,
    pub extended_pic_struct: u16,
    pub time_stamp_calc: u16,
    _rest: [u8; 20],
}

#[repr(C, packed(4))]
#[derive(Debug, Clone, Copy)]
pub struct InfoVpp {
    _reserved: [u32; 8],
    pub input: FrameInfo,
    pub output: FrameInfo,
}

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub union Infos {
    pub mfx: InfoMfx,
    pub vpp: InfoVpp,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VideoParam {
    pub alloc_id: u32,
    _reserved: [u32; 2],
    _reserved3: u16,
    pub async_depth: u16,
    pub infos: Infos,
    pub protected: u16,
    pub io_pattern: u16,
    pub ext_param: *mut *mut ExtBuffer,
    pub num_ext_param: u16,
    _reserved2: u16,
}

impl Default for VideoParam {
    fn default() -> Self {
        // SAFETY: 全て整数と整数の配列と null ポインターで、全ビット 0 は正しい値。
        unsafe { std::mem::zeroed() }
    }
}

impl std::fmt::Debug for VideoParam {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("VideoParam")
    }
}

impl VideoParam {
    pub fn mfx(&self) -> InfoMfx {
        // SAFETY: どちらの解釈も整数だけで、どのビット列も正しい値。
        unsafe { self.infos.mfx }
    }

    pub fn mfx_mut(&mut self) -> &mut InfoMfx {
        // SAFETY: 同上。
        unsafe { &mut self.infos.mfx }
    }

    pub fn vpp_mut(&mut self) -> &mut InfoVpp {
        // SAFETY: 同上。
        unsafe { &mut self.infos.vpp }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BitstreamHead {
    _encrypted: *mut c_void,
    pub ext_param: *mut *mut ExtBuffer,
    pub num_ext_param: u16,
    _reserved1: u16,
    pub codec_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union BitstreamHeadOrReserved {
    pub head: BitstreamHead,
    _reserved: [u32; 6],
}

/// 64 ビットの整数を持つ構造体は、32 ビットの Linux では 4 バイト境界で詰める（`MFX_PACK_BEGIN_STRUCT_W_L_TYPE`）。
#[cfg_attr(not(all(target_pointer_width = "32", not(windows))), repr(C))]
#[cfg_attr(all(target_pointer_width = "32", not(windows)), repr(C, packed(4)))]
#[derive(Clone, Copy)]
pub struct Bitstream {
    pub head: BitstreamHeadOrReserved,
    pub decode_time_stamp: i64,
    pub time_stamp: u64,
    pub data: *mut u8,
    pub data_offset: u32,
    pub data_length: u32,
    pub max_length: u32,
    pub pic_struct: u16,
    pub frame_type: u16,
    pub data_flag: u16,
    _reserved2: u16,
}

impl std::fmt::Debug for Bitstream {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Bitstream")
    }
}

impl Default for Bitstream {
    fn default() -> Self {
        // SAFETY: 全て整数と null ポインターで、全ビット 0 は正しい値。
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union FrameDataExt {
    pub ext_param: *mut *mut ExtBuffer,
    _reserved2: u64,
}

#[cfg_attr(not(all(target_pointer_width = "32", not(windows))), repr(C))]
#[cfg_attr(all(target_pointer_width = "32", not(windows)), repr(C, packed(4)))]
#[derive(Clone, Copy)]
pub struct FrameData {
    pub ext: FrameDataExt,
    pub num_ext_param: u16,
    _reserved: [u16; 9],
    pub mem_type: u16,
    pub pitch_high: u16,
    pub time_stamp: u64,
    pub frame_order: u32,
    pub locked: u16,
    pub pitch_low: u16,
    pub y: *mut u8,
    pub uv: *mut u8,
    pub v: *mut u8,
    pub a: *mut u8,
    pub mem_id: *mut c_void,
    pub corrupted: u16,
    pub data_flag: u16,
}

impl std::fmt::Debug for FrameData {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("FrameData")
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union FrameInterface {
    interface: *mut c_void,
    _reserved: [u32; 2],
}

#[cfg_attr(not(all(target_pointer_width = "32", not(windows))), repr(C))]
#[cfg_attr(all(target_pointer_width = "32", not(windows)), repr(C, packed(4)))]
#[derive(Clone, Copy)]
pub struct FrameSurface1 {
    frame_interface: FrameInterface,
    _version: u16,
    _reserved1: [u16; 3],
    pub info: FrameInfo,
    pub data: FrameData,
}

impl std::fmt::Debug for FrameSurface1 {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("FrameSurface1")
    }
}

#[cfg_attr(not(all(windows, feature = "d3d12")), allow(dead_code))]
impl FrameSurface1 {
    pub fn frame_interface(&self) -> *mut c_void {
        // SAFETY: ランタイムが書くのはポインターで、どのビット列もポインターとして読める（使うのは空でないとき）。
        unsafe { self.frame_interface.interface }
    }
}

impl Default for FrameSurface1 {
    fn default() -> Self {
        // SAFETY: 全て整数と null ポインターで、全ビット 0 は正しい値。
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C, packed(4))]
#[derive(Debug, Clone, Copy)]
pub struct FrameAllocRequest {
    _alloc_id: u32,
    _reserved3: [u32; 3],
    pub info: FrameInfo,
    pub kind: u16,
    pub num_frame_min: u16,
    pub num_frame_suggested: u16,
    _reserved2: u16,
}

impl Default for FrameAllocRequest {
    fn default() -> Self {
        // SAFETY: 全て整数で、全ビット 0 は正しい値。
        unsafe { std::mem::zeroed() }
    }
}

/// Media SDK のランタイムで HEVC をデコードするのに読み込むプラグイン。
pub const PLUGIN_HEVC_DECODE_HW: [u8; 16] = [
    0x33, 0xa6, 0x1c, 0x0b, 0x4c, 0x27, 0x45, 0x4c, 0xa8, 0xd8, 0x5d, 0xde, 0x75, 0x7c, 0x6f, 0x8e,
];

#[derive(Debug, Clone, Copy)]
pub struct Entries {
    pub init_ex: Option<unsafe extern "C" fn(InitParam, *mut Session) -> Status>,
    pub initialize: Option<unsafe extern "C" fn(InitializationParam, *mut Session) -> Status>,
    pub close: unsafe extern "C" fn(Session) -> Status,
    #[cfg_attr(windows, allow(dead_code))]
    pub set_handle: unsafe extern "C" fn(Session, i32, *mut c_void) -> Status,
    pub sync_operation: unsafe extern "C" fn(Session, SyncPoint, u32) -> Status,
    pub user_load: Option<unsafe extern "C" fn(Session, *const [u8; 16], u32) -> Status>,
    pub decode_query: unsafe extern "C" fn(Session, *mut VideoParam, *mut VideoParam) -> Status,
    pub decode_header: unsafe extern "C" fn(Session, *mut Bitstream, *mut VideoParam) -> Status,
    pub decode_query_io_surf:
        unsafe extern "C" fn(Session, *mut VideoParam, *mut FrameAllocRequest) -> Status,
    pub decode_init: unsafe extern "C" fn(Session, *mut VideoParam) -> Status,
    pub decode_close: unsafe extern "C" fn(Session) -> Status,
    pub decode_frame_async: unsafe extern "C" fn(
        Session,
        *mut Bitstream,
        *mut FrameSurface1,
        *mut *mut FrameSurface1,
        *mut SyncPoint,
    ) -> Status,
    pub vpp_query_io_surf:
        unsafe extern "C" fn(Session, *mut VideoParam, *mut FrameAllocRequest) -> Status,
    pub vpp_init: unsafe extern "C" fn(Session, *mut VideoParam) -> Status,
    pub vpp_close: unsafe extern "C" fn(Session) -> Status,
    pub vpp_run_frame_async: unsafe extern "C" fn(
        Session,
        *mut FrameSurface1,
        *mut FrameSurface1,
        *mut c_void,
        *mut SyncPoint,
    ) -> Status,
}

impl Entries {
    /// # Safety
    /// `object` は oneVPL か Media SDK のランタイム。
    pub unsafe fn of(object: &crate::dl::Object) -> Result<Self, crate::dl::Missing> {
        // SAFETY: 呼び出し側が保証するとおり。
        unsafe {
            Ok(Self {
                init_ex: object.find("MFXInitEx"),
                initialize: object.find("MFXInitialize"),
                close: object.symbol(b"MFXClose\0")?,
                set_handle: object.symbol(b"MFXVideoCORE_SetHandle\0")?,
                sync_operation: object.symbol(b"MFXVideoCORE_SyncOperation\0")?,
                user_load: object.find("MFXVideoUSER_Load"),
                decode_query: object.symbol(b"MFXVideoDECODE_Query\0")?,
                decode_header: object.symbol(b"MFXVideoDECODE_DecodeHeader\0")?,
                decode_query_io_surf: object.symbol(b"MFXVideoDECODE_QueryIOSurf\0")?,
                decode_init: object.symbol(b"MFXVideoDECODE_Init\0")?,
                decode_close: object.symbol(b"MFXVideoDECODE_Close\0")?,
                decode_frame_async: object.symbol(b"MFXVideoDECODE_DecodeFrameAsync\0")?,
                vpp_query_io_surf: object.symbol(b"MFXVideoVPP_QueryIOSurf\0")?,
                vpp_init: object.symbol(b"MFXVideoVPP_Init\0")?,
                vpp_close: object.symbol(b"MFXVideoVPP_Close\0")?,
                vpp_run_frame_async: object.symbol(b"MFXVideoVPP_RunFrameVPPAsync\0")?,
            })
        }
    }
}

// 構造体の大きさとフィールドのオフセット。oneVPL のヘッダーを zig cc でターゲットごとにコンパイルして測った値
// （x86_64 と aarch64 は同じ、32 ビットは Windows と Linux・ARM で W_L_TYPE の詰め方が違う）。
#[cfg(target_pointer_width = "64")]
const LAYOUT: Layout = Layout {
    init_param: (80, 16, 24, 32),
    initialization_param: (40, 16, 24),
    video_param: (208, 192, 200),
    bitstream: (72, 8, 16, 20, 40, 48, 64),
    frame_data: (96, 48, 56, 80, 88),
    frame_surface: (184, 88),
    ai_interpolation: 176,
    interface: (24, 56),
};
#[cfg(all(target_pointer_width = "32", windows))]
const LAYOUT: Layout = Layout {
    init_param: (68, 12, 16, 24),
    initialization_param: (32, 12, 16),
    video_param: (196, 188, 192),
    bitstream: (64, 4, 8, 12, 40, 44, 60),
    frame_data: (72, 48, 52, 64, 68),
    frame_surface: (160, 88),
    ai_interpolation: 144,
    interface: (16, 32),
};
#[cfg(all(target_pointer_width = "32", not(windows)))]
const LAYOUT: Layout = Layout {
    init_param: (68, 12, 16, 24),
    initialization_param: (32, 12, 16),
    video_param: (196, 188, 192),
    bitstream: (64, 4, 8, 12, 40, 44, 60),
    frame_data: (72, 48, 52, 64, 68),
    frame_surface: (156, 84),
    ai_interpolation: 144,
    interface: (16, 32),
};

/// ポインターの幅と詰め方で変わる値だけ。ほかは全ターゲットで同じ。
struct Layout {
    /// 大きさ、`ExtParam`、`NumExtParam`、`GPUCopy`。
    init_param: (usize, usize, usize, usize),
    /// 大きさ、`ExtParam`、`VendorImplID`。
    initialization_param: (usize, usize, usize),
    /// 大きさ、`ExtParam`、`NumExtParam`。
    video_param: (usize, usize, usize),
    /// 大きさ、`ExtParam`、`NumExtParam`、`CodecId`、`Data`、`DataOffset`、`DataFlag`。
    bitstream: (usize, usize, usize, usize, usize, usize, usize),
    /// 大きさ、`Y`、`UV`、`MemId`、`Corrupted`。
    frame_data: (usize, usize, usize, usize, usize),
    /// 大きさ、`Data`。
    frame_surface: (usize, usize),
    ai_interpolation: usize,
    /// `Release`、`GetNativeHandle`。
    interface: (usize, usize),
}

const _: () = {
    use std::mem::offset_of;
    let l = LAYOUT;
    assert!(size_of::<FrameInfo>() == 68);
    assert!(offset_of!(FrameInfo, bit_depth_luma) == 18);
    assert!(offset_of!(FrameInfo, shift) == 22);
    assert!(offset_of!(FrameInfo, fourcc) == 32);
    assert!(offset_of!(FrameInfo, width) == 36);
    assert!(offset_of!(FrameInfo, crop_x) == 40);
    assert!(offset_of!(FrameInfo, crop_h) == 46);
    assert!(offset_of!(FrameInfo, frame_rate_ext_n) == 48);
    assert!(offset_of!(FrameInfo, frame_rate_ext_d) == 52);
    assert!(offset_of!(FrameInfo, aspect_ratio_w) == 58);
    assert!(offset_of!(FrameInfo, pic_struct) == 62);
    assert!(offset_of!(FrameInfo, chroma_format) == 64);
    assert!(size_of::<InfoMfx>() == 136);
    assert!(offset_of!(InfoMfx, frame_info) == 32);
    assert!(offset_of!(InfoMfx, codec_id) == 100);
    assert!(offset_of!(InfoMfx, codec_profile) == 104);
    assert!(offset_of!(InfoMfx, decoded_order) == 110);
    assert!(offset_of!(InfoMfx, extended_pic_struct) == 112);
    assert!(offset_of!(InfoMfx, time_stamp_calc) == 114);
    assert!(size_of::<InfoVpp>() == 168);
    assert!(offset_of!(InfoVpp, input) == 32);
    assert!(offset_of!(InfoVpp, output) == 100);
    assert!(size_of::<VideoParam>() == l.video_param.0);
    assert!(offset_of!(VideoParam, async_depth) == 14);
    assert!(offset_of!(VideoParam, infos) == 16);
    assert!(offset_of!(VideoParam, protected) == 184);
    assert!(offset_of!(VideoParam, io_pattern) == 186);
    assert!(offset_of!(VideoParam, ext_param) == l.video_param.1);
    assert!(offset_of!(VideoParam, num_ext_param) == l.video_param.2);
    assert!(size_of::<Bitstream>() == l.bitstream.0);
    assert!(offset_of!(BitstreamHead, ext_param) == l.bitstream.1);
    assert!(offset_of!(BitstreamHead, num_ext_param) == l.bitstream.2);
    assert!(offset_of!(BitstreamHead, codec_id) == l.bitstream.3);
    assert!(size_of::<BitstreamHeadOrReserved>() == 24);
    assert!(offset_of!(Bitstream, decode_time_stamp) == 24);
    assert!(offset_of!(Bitstream, time_stamp) == 32);
    assert!(offset_of!(Bitstream, data) == l.bitstream.4);
    assert!(offset_of!(Bitstream, data_offset) == l.bitstream.5);
    assert!(offset_of!(Bitstream, data_flag) == l.bitstream.6);
    assert!(size_of::<FrameData>() == l.frame_data.0);
    assert!(offset_of!(FrameData, num_ext_param) == 8);
    assert!(offset_of!(FrameData, mem_type) == 28);
    assert!(offset_of!(FrameData, pitch_high) == 30);
    assert!(offset_of!(FrameData, time_stamp) == 32);
    assert!(offset_of!(FrameData, frame_order) == 40);
    assert!(offset_of!(FrameData, locked) == 44);
    assert!(offset_of!(FrameData, pitch_low) == 46);
    assert!(offset_of!(FrameData, y) == l.frame_data.1);
    assert!(offset_of!(FrameData, uv) == l.frame_data.2);
    assert!(offset_of!(FrameData, mem_id) == l.frame_data.3);
    assert!(offset_of!(FrameData, corrupted) == l.frame_data.4);
    assert!(size_of::<FrameSurface1>() == l.frame_surface.0);
    assert!(offset_of!(FrameSurface1, info) == 16);
    assert!(offset_of!(FrameSurface1, data) == l.frame_surface.1);
    assert!(size_of::<FrameAllocRequest>() == 92);
    assert!(offset_of!(FrameAllocRequest, info) == 16);
    assert!(offset_of!(FrameAllocRequest, kind) == 84);
    assert!(offset_of!(FrameAllocRequest, num_frame_min) == 86);
    assert!(offset_of!(FrameAllocRequest, num_frame_suggested) == 88);
    assert!(size_of::<ExtBuffer>() == 8);
    assert!(size_of::<ExtVppDeinterlacing>() == 32);
    assert!(offset_of!(ExtVppDeinterlacing, mode) == 8);
    assert!(offset_of!(ExtVppDeinterlacing, telecine_location) == 12);
    assert!(size_of::<Version>() == 4);
    assert!(size_of::<ExtVppAiFrameInterpolation>() == l.ai_interpolation);
    assert!(offset_of!(ExtVppAiFrameInterpolation, mode) == 8);
    assert!(offset_of!(ExtVppAiFrameInterpolation, enable_scd) == 12);
    assert!(size_of::<Variant>() == 16);
    assert!(offset_of!(Variant, kind) == 4);
    assert!(offset_of!(Variant, data) == 8);
    assert!(size_of::<InitParam>() == l.init_param.0);
    assert!(offset_of!(InitParam, version) == 4);
    assert!(offset_of!(InitParam, external_threads) == 8);
    assert!(offset_of!(InitParam, ext) + offset_of!(InitExt, ext_param) == l.init_param.1);
    assert!(offset_of!(InitParam, ext) + offset_of!(InitExt, num_ext_param) == l.init_param.2);
    assert!(offset_of!(InitParam, gpu_copy) == l.init_param.3);
    assert!(size_of::<InitializationParam>() == l.initialization_param.0);
    assert!(offset_of!(InitializationParam, num_ext_param) == 10);
    assert!(offset_of!(InitializationParam, ext_param) == l.initialization_param.1);
    assert!(offset_of!(InitializationParam, vendor_impl_id) == l.initialization_param.2);
    assert!(INTERFACE_RELEASE == l.interface.0);
    assert!(INTERFACE_GET_NATIVE_HANDLE == l.interface.1);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourccs_match_the_header() {
        assert_eq!(CODEC_AVC, 541_283_905);
        assert_eq!(CODEC_HEVC, 1_129_727_304);
        assert_eq!(CODEC_MPEG2, 843_534_413);
        assert_eq!(FOURCC_NV12, 842_094_158);
        assert_eq!(FOURCC_P010, 808_530_000);
        assert_eq!(EXTBUFF_VPP_DEINTERLACING, 1_229_213_782);
        assert_eq!(EXTBUFF_VPP_AI_FRAME_INTERPOLATION, 1_229_341_014);
    }
}
