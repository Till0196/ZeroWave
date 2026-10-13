// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! V4L2 and Media Request uAPI, transcribed. ioctl numbers derive from structure sizes,
//! which differ on 32-bit; the tests hold both word sizes against what a C compiler makes of
//! the kernel headers.

#![allow(dead_code)]

use std::ffi::{c_int, c_ulong, c_void};

const NRBITS: u32 = 8;
const TYPEBITS: u32 = 8;
const SIZEBITS: u32 = 14;
const NRSHIFT: u32 = 0;
const TYPESHIFT: u32 = NRSHIFT + NRBITS;
const SIZESHIFT: u32 = TYPESHIFT + TYPEBITS;
const DIRSHIFT: u32 = SIZESHIFT + SIZEBITS;
const DIR_WRITE: u32 = 1;
const DIR_READ: u32 = 2;

const fn ioc(dir: u32, kind: u8, nr: u8, size: usize) -> c_ulong {
    ((dir << DIRSHIFT)
        | ((kind as u32) << TYPESHIFT)
        | ((nr as u32) << NRSHIFT)
        | ((size as u32) << SIZESHIFT)) as c_ulong
}

pub const fn ior<T>(kind: u8, nr: u8) -> c_ulong {
    ioc(DIR_READ, kind, nr, size_of::<T>())
}

pub const fn iow<T>(kind: u8, nr: u8) -> c_ulong {
    ioc(DIR_WRITE, kind, nr, size_of::<T>())
}

pub const fn iowr<T>(kind: u8, nr: u8) -> c_ulong {
    ioc(DIR_WRITE | DIR_READ, kind, nr, size_of::<T>())
}

pub const fn io(kind: u8, nr: u8) -> c_ulong {
    ioc(0, kind, nr, 0)
}

// `ioctl` is variadic: a variadic and a fixed call differ in ABI on some targets.
unsafe extern "C" {
    #[link_name = "ioctl"]
    pub fn ioctl_raw(fd: c_int, request: c_ulong, ...) -> c_int;
}

pub const VIDEO_MAX_PLANES: usize = 8;

pub const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
pub const V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE: u32 = 9;
pub const V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE: u32 = 10;
pub const V4L2_MEMORY_MMAP: u32 = 1;
pub const V4L2_MEMORY_DMABUF: u32 = 4;

pub const V4L2_FIELD_ANY: u32 = 0;
pub const V4L2_FIELD_NONE: u32 = 1;
pub const V4L2_FIELD_TOP: u32 = 2;
pub const V4L2_FIELD_BOTTOM: u32 = 3;
pub const V4L2_FIELD_INTERLACED: u32 = 4;
pub const V4L2_FIELD_SEQ_TB: u32 = 5;
pub const V4L2_FIELD_SEQ_BT: u32 = 6;
pub const V4L2_FIELD_ALTERNATE: u32 = 7;
/// Both fields woven, top first: what `bcm2835-codec` answers for terrestrial MPEG-2.
pub const V4L2_FIELD_INTERLACED_TB: u32 = 8;
pub const V4L2_FIELD_INTERLACED_BT: u32 = 9;

pub const V4L2_CAP_VIDEO_M2M_MPLANE: u32 = 0x0000_4000;
pub const V4L2_CAP_VIDEO_M2M: u32 = 0x0000_8000;
pub const V4L2_CAP_STREAMING: u32 = 0x0400_0000;
pub const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;

pub const V4L2_FMT_FLAG_COMPRESSED: u32 = 0x0001;

pub const V4L2_BUF_CAP_SUPPORTS_REQUESTS: u32 = 1 << 3;
pub const V4L2_BUF_CAP_SUPPORTS_M2M_HOLD_CAPTURE_BUF: u32 = 1 << 5;

pub const V4L2_BUF_FLAG_DONE: u32 = 0x0000_0004;
pub const V4L2_BUF_FLAG_ERROR: u32 = 0x0000_0040;
pub const V4L2_BUF_FLAG_M2M_HOLD_CAPTURE_BUF: u32 = 0x0000_0200;
pub const V4L2_BUF_FLAG_LAST: u32 = 0x0010_0000;
pub const V4L2_BUF_FLAG_REQUEST_FD: u32 = 0x0080_0000;

/// Valid only after the source change.
pub const V4L2_CID_MIN_BUFFERS_FOR_CAPTURE: u32 = 0x0098_0927;

pub const V4L2_CTRL_WHICH_CUR_VAL: u32 = 0;
pub const V4L2_CTRL_WHICH_REQUEST_VAL: u32 = 0x0f01_0000;

pub const V4L2_CTRL_FLAG_DISABLED: u32 = 0x0001;
pub const V4L2_CTRL_FLAG_HAS_PAYLOAD: u32 = 0x0100;
pub const V4L2_CTRL_MAX_DIMS: usize = 4;

pub const V4L2_EVENT_EOS: u32 = 2;
pub const V4L2_EVENT_SOURCE_CHANGE: u32 = 5;
pub const V4L2_EVENT_SRC_CH_RESOLUTION: u32 = 1 << 0;

pub const V4L2_DEC_CMD_START: u32 = 0;
pub const V4L2_DEC_CMD_STOP: u32 = 1;

pub const fn fourcc(code: &[u8; 4]) -> u32 {
    (code[0] as u32) | ((code[1] as u32) << 8) | ((code[2] as u32) << 16) | ((code[3] as u32) << 24)
}

pub const V4L2_PIX_FMT_MPEG2_SLICE: u32 = fourcc(b"MG2S");
pub const V4L2_PIX_FMT_H264_SLICE: u32 = fourcc(b"S264");
pub const V4L2_PIX_FMT_HEVC_SLICE: u32 = fourcc(b"S265");

pub const V4L2_PIX_FMT_MPEG2: u32 = fourcc(b"MPG2");
pub const V4L2_PIX_FMT_H264: u32 = fourcc(b"H264");
pub const V4L2_PIX_FMT_HEVC: u32 = fourcc(b"HEVC");

pub const V4L2_PIX_FMT_YUV420: u32 = fourcc(b"YU12");
pub const V4L2_PIX_FMT_NV12: u32 = fourcc(b"NV12");
pub const V4L2_PIX_FMT_NV12M: u32 = fourcc(b"NM12");
pub const V4L2_PIX_FMT_P010: u32 = fourcc(b"P010");
pub const V4L2_PIX_FMT_NV15: u32 = fourcc(b"NV15");
/// Broadcom SAND (128 byte columns), Raspberry Pi kernel only.
pub const V4L2_PIX_FMT_NV12_COL128: u32 = fourcc(b"NC12");
pub const V4L2_PIX_FMT_NV12_10_COL128: u32 = fourcc(b"NC30");

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    pub driver: [u8; 16],
    pub card: [u8; 32],
    pub bus_info: [u8; 32],
    pub version: u32,
    pub capabilities: u32,
    pub device_caps: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FmtDesc {
    pub index: u32,
    pub kind: u32,
    pub flags: u32,
    pub description: [u8; 32],
    pub pixelformat: u32,
    pub mbus_code: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaneFormat {
    pub sizeimage: u32,
    /// Row stride; for `NV12_COL128` the column height in lines instead.
    pub bytesperline: u32,
    pub reserved: [u16; 6],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PixFormatMplane {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub colorspace: u32,
    pub plane_fmt: [PlaneFormat; VIDEO_MAX_PLANES],
    pub num_planes: u8,
    pub flags: u8,
    pub ycbcr_enc: u8,
    pub quantization: u8,
    pub xfer_func: u8,
    pub reserved: [u8; 7],
}

impl Default for PixFormatMplane {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            pixelformat: 0,
            field: V4L2_FIELD_ANY,
            colorspace: 0,
            plane_fmt: [PlaneFormat::default(); VIDEO_MAX_PLANES],
            num_planes: 0,
            flags: 0,
            ycbcr_enc: 0,
            quantization: 0,
            xfer_func: 0,
            reserved: [0; 7],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PixFormat {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub private: u32,
    pub flags: u32,
    pub ycbcr_enc: u32,
    pub quantization: u32,
    pub xfer_func: u32,
}

impl PixFormat {
    pub fn to_mplane(self) -> PixFormatMplane {
        let mut out = PixFormatMplane {
            width: self.width,
            height: self.height,
            pixelformat: self.pixelformat,
            field: self.field,
            colorspace: self.colorspace,
            num_planes: 1,
            flags: self.flags as u8,
            ycbcr_enc: self.ycbcr_enc as u8,
            quantization: self.quantization as u8,
            xfer_func: self.xfer_func as u8,
            ..PixFormatMplane::default()
        };
        out.plane_fmt[0].bytesperline = self.bytesperline;
        out.plane_fmt[0].sizeimage = self.sizeimage;
        out
    }

    pub fn of_mplane(mp: &PixFormatMplane) -> Self {
        Self {
            width: mp.width,
            height: mp.height,
            pixelformat: mp.pixelformat,
            field: mp.field,
            bytesperline: mp.plane_fmt[0].bytesperline,
            sizeimage: mp.plane_fmt[0].sizeimage,
            colorspace: mp.colorspace,
            private: 0,
            flags: u32::from(mp.flags),
            ycbcr_enc: u32::from(mp.ycbcr_enc),
            quantization: u32::from(mp.quantization),
            xfer_func: u32::from(mp.xfer_func),
        }
    }
}

/// The union aligns to a pointer: 208 bytes on 64-bit, 204 on 32-bit.
/// A wrong size gives `ENOTTY` on `S_FMT`.
#[cfg_attr(target_pointer_width = "64", repr(C, align(8)))]
#[cfg_attr(not(target_pointer_width = "64"), repr(C, align(4)))]
#[derive(Debug, Clone, Copy)]
pub struct Format {
    pub kind: u32,
    #[cfg(target_pointer_width = "64")]
    pad: u32,
    pub pix_mp: PixFormatMplane,
    pub padding: [u8; 8],
}

impl Format {
    pub fn pix(&self) -> PixFormat {
        // SAFETY: both arms start at the same offset; `PixFormat` is shorter and any bits do.
        unsafe { std::ptr::read_unaligned((&raw const self.pix_mp).cast::<PixFormat>()) }
    }

    pub fn set_pix(&mut self, pix: PixFormat) {
        // SAFETY: as in `pix`; the bytes after the 48 stay as they were.
        unsafe { std::ptr::write_unaligned((&raw mut self.pix_mp).cast::<PixFormat>(), pix) }
    }

    pub fn new(kind: u32) -> Self {
        Self {
            kind,
            #[cfg(target_pointer_width = "64")]
            pad: 0,
            pix_mp: PixFormatMplane::default(),
            padding: [0; 8],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestBuffers {
    pub count: u32,
    pub kind: u32,
    pub memory: u32,
    pub capabilities: u32,
    pub flags: u8,
    pub reserved: [u8; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CreateBuffers {
    pub index: u32,
    pub count: u32,
    pub memory: u32,
    /// `v4l2_format` の共用体はポインターを含むので 64 ビットでは 8 に揃う。
    #[cfg(target_pointer_width = "64")]
    pad: u32,
    pub format: Format,
    pub capabilities: u32,
    pub flags: u32,
    pub max_num_buffers: u32,
    pub reserved: [u32; 5],
}

impl CreateBuffers {
    pub fn new(count: u32, memory: u32, format: Format) -> Self {
        Self {
            index: 0,
            count,
            memory,
            #[cfg(target_pointer_width = "64")]
            pad: 0,
            format,
            capabilities: 0,
            flags: 0,
            max_num_buffers: 0,
            reserved: [0; 5],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union PlaneMemory {
    pub mem_offset: u32,
    pub userptr: usize,
    pub fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub bytesused: u32,
    pub length: u32,
    pub m: PlaneMemory,
    pub data_offset: u32,
    pub reserved: [u32; 11],
}

impl Default for Plane {
    fn default() -> Self {
        Self {
            bytesused: 0,
            length: 0,
            m: PlaneMemory { mem_offset: 0 },
            data_offset: 0,
            reserved: [0; 11],
        }
    }
}

/// Two `long long` also on armhf; two `isize` would change `VIDIOC_QBUF` on 32-bit.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

impl Timeval {
    /// As the kernel's `v4l2_timeval_to_ns`; how a stateless control names a reference.
    pub fn to_ns(self) -> u64 {
        (self.tv_sec as u64)
            .wrapping_mul(1_000_000_000)
            .wrapping_add((self.tv_usec as u64).wrapping_mul(1_000))
    }

    pub fn from_ns(ns: u64) -> Self {
        Self {
            tv_sec: (ns / 1_000_000_000) as i64,
            tv_usec: ((ns % 1_000_000_000) / 1_000) as i64,
        }
    }
}

/// すべて 0 を既定にする。フィールドは整数と整数の配列だけの `repr(C)`。
macro_rules! zeroed_default {
    ($($name:ident),* $(,)?) => {$(
        impl Default for $name {
            fn default() -> Self {
                // SAFETY: 整数と整数の配列だけなのですべて 0 が正しい値。
                unsafe { std::mem::zeroed() }
            }
        }
    )*};
}

zeroed_default!(QueryExtCtrl, Event, MediaV2Interface);

macro_rules! debug_as_bits {
    ($name:ident, $arm:ident) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                // SAFETY: every arm is an integer or a pointer.
                let bits = unsafe { self.$arm };
                write!(out, "{}({bits:?})", stringify!($name))
            }
        }
    };
}

debug_as_bits!(PlaneMemory, userptr);
debug_as_bits!(BufferMemory, userptr);
debug_as_bits!(ExtControlValue, value64);

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timecode {
    pub kind: u32,
    pub flags: u32,
    pub frames: u8,
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub userbits: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union BufferMemory {
    pub offset: u32,
    pub userptr: usize,
    pub planes: *mut Plane,
    pub fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Buffer {
    pub index: u32,
    pub kind: u32,
    pub bytesused: u32,
    pub flags: u32,
    pub field: u32,
    pub timestamp: Timeval,
    pub timecode: Timecode,
    pub sequence: u32,
    pub memory: u32,
    pub m: BufferMemory,
    pub length: u32,
    pub reserved2: u32,
    pub request_fd: i32,
}

impl Buffer {
    pub fn new(kind: u32, index: u32) -> Self {
        Self {
            index,
            kind,
            bytesused: 0,
            flags: 0,
            field: V4L2_FIELD_ANY,
            timestamp: Timeval::default(),
            timecode: Timecode::default(),
            sequence: 0,
            memory: V4L2_MEMORY_MMAP,
            m: BufferMemory { offset: 0 },
            length: 0,
            reserved2: 0,
            request_fd: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ExportBuffer {
    pub kind: u32,
    pub index: u32,
    pub plane: u32,
    pub flags: u32,
    pub fd: i32,
    pub reserved: [u32; 11],
}

/// Packed: 20 bytes, not 24.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub union ExtControlValue {
    pub value: i32,
    pub value64: i64,
    pub ptr: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExtControl {
    pub id: u32,
    pub size: u32,
    pub reserved2: [u32; 1],
    pub value: ExtControlValue,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExtControls {
    pub which: u32,
    pub count: u32,
    pub error_idx: u32,
    pub request_fd: i32,
    pub reserved: [u32; 1],
    pub controls: *mut ExtControl,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct QueryExtCtrl {
    pub id: u32,
    pub kind: u32,
    pub name: [u8; 32],
    pub minimum: i64,
    pub maximum: i64,
    pub step: u64,
    pub default_value: i64,
    pub flags: u32,
    /// Element size; held against `size_of` to catch a kernel with another layout.
    pub elem_size: u32,
    pub elems: u32,
    pub nr_of_dims: u32,
    pub dims: [u32; V4L2_CTRL_MAX_DIMS],
    pub reserved: [u32; 32],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EventSourceChange {
    pub changes: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub kind: u32,
    /// The union is 8-byte aligned (`__s64`), so at offset 8 on both word sizes.
    pad: u32,
    pub u: [u8; 64],
    pub pending: u32,
    pub sequence: u32,
    pub timestamp: [i64; 2],
    pub id: u32,
    pub reserved: [u32; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Control {
    pub id: u32,
    pub value: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

/// How the visible picture is told from the buffer (1088 rows for 1080).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Selection {
    pub kind: u32,
    pub target: u32,
    pub flags: u32,
    pub r: Rect,
    pub reserved: [u32; 9],
}

pub const V4L2_SEL_TGT_COMPOSE: u32 = 0x0100;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct EventSubscription {
    pub kind: u32,
    pub id: u32,
    pub flags: u32,
    pub reserved: [u32; 5],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DecoderCmd {
    pub cmd: u32,
    pub flags: u32,
    pub raw: [u64; 8],
}

pub const VIDIOC_QUERYCAP: c_ulong = ior::<Capability>(b'V', 0);
pub const VIDIOC_ENUM_FMT: c_ulong = iowr::<FmtDesc>(b'V', 2);
pub const VIDIOC_G_FMT: c_ulong = iowr::<Format>(b'V', 4);
pub const VIDIOC_S_FMT: c_ulong = iowr::<Format>(b'V', 5);
pub const VIDIOC_REQBUFS: c_ulong = iowr::<RequestBuffers>(b'V', 8);
pub const VIDIOC_CREATE_BUFS: c_ulong = iowr::<CreateBuffers>(b'V', 92);
pub const VIDIOC_QUERYBUF: c_ulong = iowr::<Buffer>(b'V', 9);
pub const VIDIOC_QBUF: c_ulong = iowr::<Buffer>(b'V', 15);
pub const VIDIOC_EXPBUF: c_ulong = iowr::<ExportBuffer>(b'V', 16);
pub const VIDIOC_DQBUF: c_ulong = iowr::<Buffer>(b'V', 17);
pub const VIDIOC_STREAMON: c_ulong = iow::<c_int>(b'V', 18);
pub const VIDIOC_STREAMOFF: c_ulong = iow::<c_int>(b'V', 19);
pub const VIDIOC_G_CTRL: c_ulong = iowr::<Control>(b'V', 27);
pub const VIDIOC_G_EXT_CTRLS: c_ulong = iowr::<ExtControls>(b'V', 71);
pub const VIDIOC_S_EXT_CTRLS: c_ulong = iowr::<ExtControls>(b'V', 72);
pub const VIDIOC_TRY_EXT_CTRLS: c_ulong = iowr::<ExtControls>(b'V', 73);
pub const VIDIOC_SUBSCRIBE_EVENT: c_ulong = iow::<EventSubscription>(b'V', 90);
pub const VIDIOC_DQEVENT: c_ulong = ior::<Event>(b'V', 89);
pub const VIDIOC_G_SELECTION: c_ulong = iowr::<Selection>(b'V', 94);
pub const VIDIOC_DECODER_CMD: c_ulong = iowr::<DecoderCmd>(b'V', 96);
pub const VIDIOC_QUERY_EXT_CTRL: c_ulong = iowr::<QueryExtCtrl>(b'V', 103);

pub const MEDIA_INTF_T_V4L_VIDEO: u32 = 0x0000_0200;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaIntfDevnode {
    pub major: u32,
    pub minor: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MediaV2Interface {
    pub id: u32,
    pub intf_type: u32,
    pub flags: u32,
    pub reserved: [u32; 9],
    pub devnode: MediaIntfDevnode,
    pub rest: [u32; 14],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaV2Topology {
    pub topology_version: u64,
    pub num_entities: u32,
    pub reserved1: u32,
    pub ptr_entities: u64,
    pub num_interfaces: u32,
    pub reserved2: u32,
    pub ptr_interfaces: u64,
    pub num_pads: u32,
    pub reserved3: u32,
    pub ptr_pads: u64,
    pub num_links: u32,
    pub reserved4: u32,
    pub ptr_links: u64,
}

pub const MEDIA_IOC_G_TOPOLOGY: c_ulong = iowr::<MediaV2Topology>(b'|', 4);
pub const MEDIA_IOC_REQUEST_ALLOC: c_ulong = ior::<i32>(b'|', 5);
pub const MEDIA_REQUEST_IOC_QUEUE: c_ulong = io(b'|', 0x80);
pub const MEDIA_REQUEST_IOC_REINIT: c_ulong = io(b'|', 0x81);

pub const DRM_FORMAT_R8: u32 = fourcc(b"R8  ");
pub const DRM_FORMAT_GR88: u32 = fourcc(b"GR88");
pub const DRM_FORMAT_R16: u32 = fourcc(b"R16 ");
pub const DRM_FORMAT_GR1616: u32 = fourcc(b"GR32");
pub const DRM_FORMAT_NV12: u32 = fourcc(b"NV12");
pub const DRM_FORMAT_P010: u32 = fourcc(b"P010");
pub const DRM_FORMAT_P030: u32 = fourcc(b"P030");
pub const DRM_FORMAT_NV15: u32 = fourcc(b"NV15");

pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
const DRM_FORMAT_MOD_VENDOR_BROADCOM: u64 = 0x07;

/// The column height, which V4L2 gives as the capture plane's `bytesperline`.
pub const fn sand128_col_height(height: u32) -> u64 {
    (DRM_FORMAT_MOD_VENDOR_BROADCOM << 56) | (((height as u64) << 8) | 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_structures_are_the_size_the_kernel_declares() {
        assert_eq!(size_of::<Capability>(), 104);
        assert_eq!(size_of::<FmtDesc>(), 64);
        assert_eq!(size_of::<Format>(), 208);
        assert_eq!(size_of::<CreateBuffers>(), 256);
        assert_eq!(std::mem::offset_of!(CreateBuffers, format), 16);
        assert_eq!(size_of::<PixFormatMplane>(), 192);
        assert_eq!(size_of::<PixFormat>(), 48);
        assert_eq!(size_of::<PlaneFormat>(), 20);
        assert_eq!(size_of::<RequestBuffers>(), 20);
        assert_eq!(size_of::<Plane>(), 64);
        assert_eq!(size_of::<Buffer>(), 88);
        assert_eq!(size_of::<ExportBuffer>(), 64);
        assert_eq!(size_of::<ExtControl>(), 20);
        assert_eq!(size_of::<ExtControls>(), 32);
        assert_eq!(size_of::<QueryExtCtrl>(), 232);
        assert_eq!(size_of::<Event>(), 136);
        assert_eq!(size_of::<EventSubscription>(), 32);
        assert_eq!(size_of::<Selection>(), 64);
        assert_eq!(size_of::<MediaV2Topology>(), 72);
        assert_eq!(std::mem::offset_of!(Event, u), 8);
        assert_eq!(std::mem::offset_of!(Event, pending), 72);
        assert_eq!(std::mem::offset_of!(Event, timestamp), 80);
        assert_eq!(std::mem::offset_of!(Event, id), 96);
        assert_eq!(size_of::<MediaV2Interface>(), 112);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_fields_are_where_the_kernel_puts_them() {
        assert_eq!(std::mem::offset_of!(Buffer, timestamp), 24);
        assert_eq!(std::mem::offset_of!(Buffer, timecode), 40);
        assert_eq!(std::mem::offset_of!(Buffer, sequence), 56);
        assert_eq!(std::mem::offset_of!(Buffer, memory), 60);
        assert_eq!(std::mem::offset_of!(Buffer, m), 64);
        assert_eq!(std::mem::offset_of!(Buffer, length), 72);
        assert_eq!(std::mem::offset_of!(Buffer, request_fd), 80);
        assert_eq!(std::mem::offset_of!(ExtControls, request_fd), 12);
        assert_eq!(std::mem::offset_of!(ExtControls, controls), 24);
        assert_eq!(std::mem::offset_of!(QueryExtCtrl, flags), 72);
        assert_eq!(std::mem::offset_of!(QueryExtCtrl, elem_size), 76);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_ioctl_numbers_are_what_the_headers_compute() {
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_ENUM_FMT, 0xc040_5602);
        assert_eq!(VIDIOC_G_FMT, 0xc0d0_5604);
        assert_eq!(VIDIOC_S_FMT, 0xc0d0_5605);
        assert_eq!(VIDIOC_REQBUFS, 0xc014_5608);
        assert_eq!(VIDIOC_CREATE_BUFS, 0xc100_565c);
        assert_eq!(VIDIOC_QUERYBUF, 0xc058_5609);
        assert_eq!(VIDIOC_QBUF, 0xc058_560f);
        assert_eq!(VIDIOC_EXPBUF, 0xc040_5610);
        assert_eq!(VIDIOC_DQBUF, 0xc058_5611);
        assert_eq!(VIDIOC_STREAMON, 0x4004_5612);
        assert_eq!(VIDIOC_STREAMOFF, 0x4004_5613);
        assert_eq!(VIDIOC_G_EXT_CTRLS, 0xc020_5647);
        assert_eq!(VIDIOC_S_EXT_CTRLS, 0xc020_5648);
        assert_eq!(VIDIOC_TRY_EXT_CTRLS, 0xc020_5649);
        assert_eq!(VIDIOC_DQEVENT, 0x8088_5659);
        assert_eq!(VIDIOC_SUBSCRIBE_EVENT, 0x4020_565a);
        assert_eq!(VIDIOC_G_CTRL, 0xc008_561b);
        assert_eq!(VIDIOC_G_SELECTION, 0xc040_565e);
        assert_eq!(VIDIOC_DECODER_CMD, 0xc048_5660);
        assert_eq!(VIDIOC_QUERY_EXT_CTRL, 0xc0e8_5667);
        assert_eq!(MEDIA_IOC_G_TOPOLOGY, 0xc048_7c04);
        assert_eq!(MEDIA_IOC_REQUEST_ALLOC, 0x8004_7c05);
        assert_eq!(MEDIA_REQUEST_IOC_QUEUE, 0x7c80);
        assert_eq!(MEDIA_REQUEST_IOC_REINIT, 0x7c81);
    }

    /// What a 32-bit C compiler makes of the kernel headers, not derived.
    #[test]
    #[cfg(target_pointer_width = "32")]
    fn the_structures_are_what_a_32_bit_compiler_makes_of_them() {
        assert_eq!(size_of::<Capability>(), 104);
        assert_eq!(size_of::<FmtDesc>(), 64);
        assert_eq!(size_of::<Format>(), 204);
        assert_eq!(size_of::<CreateBuffers>(), 248);
        assert_eq!(size_of::<PixFormatMplane>(), 192);
        assert_eq!(size_of::<PixFormat>(), 48);
        assert_eq!(size_of::<PlaneFormat>(), 20);
        assert_eq!(size_of::<RequestBuffers>(), 20);
        assert_eq!(size_of::<Plane>(), 60);
        assert_eq!(size_of::<Buffer>(), 80);
        assert_eq!(size_of::<ExportBuffer>(), 64);
        assert_eq!(size_of::<ExtControl>(), 20);
        assert_eq!(size_of::<ExtControls>(), 24);
        assert_eq!(size_of::<QueryExtCtrl>(), 232);
        assert_eq!(size_of::<Event>(), 136);
        assert_eq!(size_of::<EventSubscription>(), 32);
        assert_eq!(size_of::<Selection>(), 64);
        assert_eq!(size_of::<DecoderCmd>(), 72);
        assert_eq!(std::mem::offset_of!(Event, u), 8);
        assert_eq!(std::mem::offset_of!(Event, pending), 72);
        assert_eq!(std::mem::offset_of!(Event, timestamp), 80);
        assert_eq!(std::mem::offset_of!(Event, id), 96);
        assert_eq!(std::mem::offset_of!(Format, pix_mp), 4);
        assert_eq!(std::mem::offset_of!(Plane, m), 8);
        assert_eq!(std::mem::offset_of!(Buffer, timestamp), 24);
        assert_eq!(std::mem::offset_of!(Buffer, timecode), 40);
        assert_eq!(std::mem::offset_of!(Buffer, m), 64);
        assert_eq!(std::mem::offset_of!(Buffer, length), 68);
        assert_eq!(std::mem::offset_of!(Buffer, request_fd), 76);
        assert_eq!(std::mem::offset_of!(ExtControls, controls), 20);
        assert_eq!(std::mem::offset_of!(Selection, r), 12);
        assert_eq!(VIDIOC_G_FMT, 0xc0cc_5604);
        assert_eq!(VIDIOC_S_FMT, 0xc0cc_5605);
        assert_eq!(VIDIOC_QUERYBUF, 0xc050_5609);
        assert_eq!(VIDIOC_QBUF, 0xc050_560f);
        assert_eq!(VIDIOC_DQBUF, 0xc050_5611);
        assert_eq!(VIDIOC_G_EXT_CTRLS, 0xc018_5647);
        assert_eq!(VIDIOC_S_EXT_CTRLS, 0xc018_5648);
        assert_eq!(VIDIOC_DQEVENT, 0x8088_5659);
        assert_eq!(VIDIOC_G_CTRL, 0xc008_561b);
        assert_eq!(VIDIOC_G_SELECTION, 0xc040_565e);
        assert_eq!(VIDIOC_DECODER_CMD, 0xc048_5660);
    }

    #[test]
    fn the_pixel_formats_are_the_codes_the_header_spells() {
        assert_eq!(V4L2_PIX_FMT_MPEG2_SLICE, 0x5332_474d);
        assert_eq!(V4L2_PIX_FMT_H264_SLICE, 0x3436_3253);
        assert_eq!(V4L2_PIX_FMT_HEVC_SLICE, 0x3536_3253);
        assert_eq!(V4L2_PIX_FMT_H264, 0x3436_3248);
        assert_eq!(V4L2_PIX_FMT_HEVC, 0x4356_4548);
        assert_eq!(V4L2_PIX_FMT_MPEG2, 0x3247_504d);
        assert_eq!(V4L2_PIX_FMT_NV12, 0x3231_564e);
        assert_eq!(V4L2_PIX_FMT_NV12_COL128, 0x3231_434e);
        assert_eq!(V4L2_PIX_FMT_NV12_10_COL128, 0x3033_434e);
        assert_eq!(DRM_FORMAT_P030, 0x3033_3050);
    }

    #[test]
    fn the_sand_modifier_carries_the_column_height() {
        assert_eq!(sand128_col_height(0), 0x0700_0000_0000_0004);
        assert_eq!(sand128_col_height(1088), 0x0700_0000_0004_4004);
        assert_eq!((sand128_col_height(1088) >> 8) & 0x00ff_ffff, 1088);
    }

    #[test]
    fn a_timestamp_survives_the_round_trip_the_kernel_makes_of_it() {
        for ns in [0u64, 1_000, 999_999_000, 1_000_000_000, 86_400_000_000_000] {
            assert_eq!(Timeval::from_ns(ns).to_ns(), ns);
        }
    }
}
