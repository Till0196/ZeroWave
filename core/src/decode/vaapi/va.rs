// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! libva, opened with `dlopen` (`libva.so.2`) so the crate builds without `libva-dev`.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};

use crate::decode::Error;
use crate::decode::mmap::Mapping;
use crate::dl::Object;

pub type VADisplay = *mut c_void;
pub type VAStatus = c_int;
pub type VASurfaceID = c_uint;
pub type VAConfigID = c_uint;
pub type VAContextID = c_uint;
pub type VABufferID = c_uint;
pub type VAImageID = c_uint;

pub const VA_STATUS_SUCCESS: VAStatus = 0;
pub const VA_INVALID_ID: c_uint = 0xffff_ffff;
pub const VA_INVALID_SURFACE: VASurfaceID = VA_INVALID_ID;

pub const VA_PROFILE_MPEG2_MAIN: c_int = 1;
pub const VA_PROFILE_H264_HIGH: c_int = 7;
pub const VA_PROFILE_HEVC_MAIN: c_int = 17;
pub const VA_PROFILE_HEVC_MAIN10: c_int = 18;
pub const VA_ENTRYPOINT_VLD: c_int = 1;

// ビデオ処理（`va_vpp.h` から移した。MIT）。
pub const VA_PROFILE_NONE: c_int = -1;
pub const VA_ENTRYPOINT_VIDEO_PROC: c_int = 10;
pub const VA_PROC_PIPELINE_PARAMETER_BUFFER_TYPE: c_int = 41;
pub const VA_PROC_FILTER_PARAMETER_BUFFER_TYPE: c_int = 42;
pub const VA_PROC_FILTER_DEINTERLACING: c_int = 2;
pub const VA_PROC_FILTER_COUNT: usize = 10;
pub const VA_PROC_DEINTERLACING_BOB: c_int = 1;
pub const VA_PROC_DEINTERLACING_MOTION_ADAPTIVE: c_int = 3;
pub const VA_PROC_DEINTERLACING_MOTION_COMPENSATED: c_int = 4;
pub const VA_PROC_DEINTERLACING_COUNT: usize = 5;
pub const VA_DEINTERLACING_BOTTOM_FIELD_FIRST: u32 = 0x0001;
pub const VA_DEINTERLACING_BOTTOM_FIELD: u32 = 0x0002;

pub const VA_CONFIG_ATTRIB_RT_FORMAT: c_int = 0;
pub const VA_RT_FORMAT_YUV420: c_uint = 0x0000_0001;
pub const VA_RT_FORMAT_YUV420_10: c_uint = 0x0000_0100;

pub const VA_SURFACE_ATTRIB_PIXEL_FORMAT: c_int = 1;
pub const VA_SURFACE_ATTRIB_SETTABLE: u32 = 0x0002;
pub const VA_GENERIC_VALUE_TYPE_INTEGER: c_int = 1;

pub const VA_FOURCC_NV12: u32 = 0x3231_564e;
pub const VA_FOURCC_P010: u32 = 0x3031_3050;

pub const VA_PICTURE_PARAMETER_BUFFER_TYPE: c_int = 0;
pub const VA_IQ_MATRIX_BUFFER_TYPE: c_int = 1;
pub const VA_SLICE_PARAMETER_BUFFER_TYPE: c_int = 4;
pub const VA_SLICE_DATA_BUFFER_TYPE: c_int = 5;
pub const VA_SLICE_DATA_FLAG_ALL: u32 = 0;

/// `0x4000000` (one nought short) is refused with `unsupported memory type`.
pub const VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2: u32 = 0x4000_0000;
pub const VA_SURFACE_ATTRIB_MEMORY_TYPE: c_int = 6;
pub const VA_SURFACE_ATTRIB_EXTERNAL_BUFFER_DESCRIPTOR: c_int = 7;
pub const VA_GENERIC_VALUE_TYPE_POINTER: c_int = 3;

pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

pub const VA_EXPORT_SURFACE_READ_ONLY: u32 = 0x0001;
pub const VA_EXPORT_SURFACE_SEPARATE_LAYERS: u32 = 0x0004;

pub const VA_PICTURE_H264_INVALID: u32 = 0x0001;
pub const VA_PICTURE_H264_TOP_FIELD: u32 = 0x0002;
pub const VA_PICTURE_H264_BOTTOM_FIELD: u32 = 0x0004;
pub const VA_PICTURE_H264_SHORT_TERM_REFERENCE: u32 = 0x0008;
pub const VA_PICTURE_H264_LONG_TERM_REFERENCE: u32 = 0x0010;

pub const VA_PICTURE_HEVC_INVALID: u32 = 0x0001;
pub const VA_PICTURE_HEVC_LONG_TERM_REFERENCE: u32 = 0x0008;
pub const VA_PICTURE_HEVC_RPS_ST_CURR_BEFORE: u32 = 0x0010;
pub const VA_PICTURE_HEVC_RPS_ST_CURR_AFTER: u32 = 0x0020;
pub const VA_PICTURE_HEVC_RPS_LT_CURR: u32 = 0x0040;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GenericValue {
    pub kind: c_int,
    pub value: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SurfaceAttrib {
    pub kind: c_int,
    pub flags: u32,
    pub value: GenericValue,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ConfigAttrib {
    pub kind: c_int,
    pub value: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PrimeObject {
    pub fd: c_int,
    pub size: u32,
    pub drm_format_modifier: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PrimeLayer {
    pub drm_format: u32,
    pub num_planes: u32,
    pub object_index: [u32; 4],
    pub offset: [u32; 4],
    pub pitch: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PrimeDescriptor {
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    pub num_objects: u32,
    pub objects: [PrimeObject; 4],
    pub num_layers: u32,
    pub layers: [PrimeLayer; 4],
}

/// `VAImageFormat`. Without the four reserved words every later field of `VAImage` is misread.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageFormat {
    pub fourcc: u32,
    pub byte_order: u32,
    pub bits_per_pixel: u32,
    pub depth: u32,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub alpha_mask: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Image {
    pub image_id: VAImageID,
    pub format: ImageFormat,
    pub buf: VABufferID,
    pub width: u16,
    pub height: u16,
    pub data_size: u32,
    pub num_planes: u32,
    pub pitches: [u32; 3],
    pub offsets: [u32; 3],
    pub num_palette_entries: c_int,
    pub entry_bytes: c_int,
    pub component_order: [c_char; 4],
    pub reserved: [u32; 4],
}

/// # Safety
/// 全ビット 0 が正しい値の型（整数、整数の配列、null で良いポインター）にだけ付ける。
pub unsafe trait Zeroed: Sized {}

/// 並べた型に [`Zeroed`] を付ける。
macro_rules! zeroed_types {
    ($($name:ty),* $(,)?) => {
        // SAFETY: 並べた型は整数・整数の配列・null で良いポインターだけ。
        $(unsafe impl $crate::decode::vaapi::va::Zeroed for $name {})*
    };
}
pub(crate) use zeroed_types;

pub fn zeroed<T: Zeroed>() -> T {
    // SAFETY: `Zeroed` の前提どおり全ビット 0 が正しい値。
    unsafe { std::mem::zeroed() }
}

zeroed_types!(Image, PipelineParameters, PipelineCaps);

impl Default for Image {
    fn default() -> Self {
        zeroed()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DeinterlacingParameters {
    pub kind: c_int,
    pub algorithm: c_int,
    pub flags: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DeinterlacingCap {
    pub kind: c_int,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorProperties {
    pub chroma_sample_location: u8,
    pub color_range: u8,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub reserved: [u8; 3],
}

/// フィールドを抜かすと後ろが全部ずれる（64 ビットで 224 バイト）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PipelineParameters {
    pub surface: VASurfaceID,
    pub surface_region: *const c_void,
    pub surface_color_standard: c_int,
    pub output_region: *const c_void,
    pub output_background_color: u32,
    pub output_color_standard: c_int,
    pub pipeline_flags: u32,
    pub filter_flags: u32,
    pub filters: *mut VABufferID,
    pub num_filters: u32,
    pub forward_references: *mut VASurfaceID,
    pub num_forward_references: u32,
    pub backward_references: *mut VASurfaceID,
    pub num_backward_references: u32,
    pub rotation_state: u32,
    pub blend_state: *const c_void,
    pub mirror_state: u32,
    pub additional_outputs: *mut VASurfaceID,
    pub num_additional_outputs: u32,
    pub input_surface_flag: u32,
    pub output_surface_flag: u32,
    pub input_color_properties: ColorProperties,
    pub output_color_properties: ColorProperties,
    pub processing_mode: c_int,
    pub output_hdr_metadata: *const c_void,
    pub reserved: [u32; 16],
}

impl Default for PipelineParameters {
    fn default() -> Self {
        // 全ビット 0 は「何もしない」の値。
        zeroed()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PipelineCaps {
    pub pipeline_flags: u32,
    pub filter_flags: u32,
    pub num_forward_references: u32,
    pub num_backward_references: u32,
    pub input_color_standards: *mut c_int,
    pub num_input_color_standards: u32,
    pub output_color_standards: *mut c_int,
    pub num_output_color_standards: u32,
    pub rotation_flags: u32,
    pub blend_flags: u32,
    pub mirror_flags: u32,
    pub num_additional_outputs: u32,
    pub num_input_pixel_formats: u32,
    pub input_pixel_format: *mut u32,
    pub num_output_pixel_formats: u32,
    pub output_pixel_format: *mut u32,
    pub max_input_width: u32,
    pub max_input_height: u32,
    pub min_input_width: u32,
    pub min_input_height: u32,
    pub max_output_width: u32,
    pub max_output_height: u32,
    pub min_output_width: u32,
    pub min_output_height: u32,
    pub reserved: [u32; 14],
}

impl Default for PipelineCaps {
    fn default() -> Self {
        zeroed()
    }
}

#[allow(clippy::type_complexity)]
struct Symbols {
    get_display_drm: unsafe extern "C" fn(c_int) -> VADisplay,
    initialize: unsafe extern "C" fn(VADisplay, *mut c_int, *mut c_int) -> VAStatus,
    terminate: unsafe extern "C" fn(VADisplay) -> VAStatus,
    vendor: unsafe extern "C" fn(VADisplay) -> *const c_char,
    error: unsafe extern "C" fn(VAStatus) -> *const c_char,
    max_entrypoints: unsafe extern "C" fn(VADisplay) -> c_int,
    query_entrypoints: unsafe extern "C" fn(VADisplay, c_int, *mut c_int, *mut c_int) -> VAStatus,
    get_config_attributes:
        unsafe extern "C" fn(VADisplay, c_int, c_int, *mut ConfigAttrib, c_int) -> VAStatus,
    create_config: unsafe extern "C" fn(
        VADisplay,
        c_int,
        c_int,
        *mut ConfigAttrib,
        c_int,
        *mut VAConfigID,
    ) -> VAStatus,
    destroy_config: unsafe extern "C" fn(VADisplay, VAConfigID) -> VAStatus,
    create_surfaces: unsafe extern "C" fn(
        VADisplay,
        c_uint,
        c_uint,
        c_uint,
        *mut VASurfaceID,
        c_uint,
        *mut SurfaceAttrib,
        c_uint,
    ) -> VAStatus,
    destroy_surfaces: unsafe extern "C" fn(VADisplay, *mut VASurfaceID, c_int) -> VAStatus,
    sync_surface: unsafe extern "C" fn(VADisplay, VASurfaceID) -> VAStatus,
    create_context: unsafe extern "C" fn(
        VADisplay,
        VAConfigID,
        c_int,
        c_int,
        c_int,
        *mut VASurfaceID,
        c_int,
        *mut VAContextID,
    ) -> VAStatus,
    destroy_context: unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus,
    create_buffer: unsafe extern "C" fn(
        VADisplay,
        VAContextID,
        c_int,
        c_uint,
        c_uint,
        *const c_void,
        *mut VABufferID,
    ) -> VAStatus,
    destroy_buffer: unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus,
    begin_picture: unsafe extern "C" fn(VADisplay, VAContextID, VASurfaceID) -> VAStatus,
    render_picture:
        unsafe extern "C" fn(VADisplay, VAContextID, *mut VABufferID, c_int) -> VAStatus,
    end_picture: unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus,
    export_surface: unsafe extern "C" fn(VADisplay, VASurfaceID, u32, u32, *mut c_void) -> VAStatus,
    derive_image: unsafe extern "C" fn(VADisplay, VASurfaceID, *mut Image) -> VAStatus,
    destroy_image: unsafe extern "C" fn(VADisplay, VAImageID) -> VAStatus,
    map_buffer: unsafe extern "C" fn(VADisplay, VABufferID, *mut *mut c_void) -> VAStatus,
    unmap_buffer: unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus,
    query_filters:
        unsafe extern "C" fn(VADisplay, VAContextID, *mut c_int, *mut c_uint) -> VAStatus,
    query_filter_caps:
        unsafe extern "C" fn(VADisplay, VAContextID, c_int, *mut c_void, *mut c_uint) -> VAStatus,
    query_pipeline_caps: unsafe extern "C" fn(
        VADisplay,
        VAContextID,
        *mut VABufferID,
        c_uint,
        *mut PipelineCaps,
    ) -> VAStatus,
}

pub struct Library {
    symbols: Symbols,
    _va: Object,
    _drm: Object,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Library")
    }
}

macro_rules! symbol {
    ($object:expr, $name:literal) => {
        // SAFETY: the type bound in `Symbols` is the signature `va.h` declares.
        unsafe { $object.symbol($name) }.map_err(|missing| Error::Unavailable(missing.0))?
    };
}

impl Library {
    pub fn load() -> Result<Self, Error> {
        let va = Object::open("libva.so.2").map_err(|missing| Error::Unavailable(missing.0))?;
        let drm =
            Object::open("libva-drm.so.2").map_err(|missing| Error::Unavailable(missing.0))?;
        let symbols = Symbols {
            get_display_drm: symbol!(drm, b"vaGetDisplayDRM\0"),
            initialize: symbol!(va, b"vaInitialize\0"),
            terminate: symbol!(va, b"vaTerminate\0"),
            vendor: symbol!(va, b"vaQueryVendorString\0"),
            error: symbol!(va, b"vaErrorStr\0"),
            max_entrypoints: symbol!(va, b"vaMaxNumEntrypoints\0"),
            query_entrypoints: symbol!(va, b"vaQueryConfigEntrypoints\0"),
            get_config_attributes: symbol!(va, b"vaGetConfigAttributes\0"),
            create_config: symbol!(va, b"vaCreateConfig\0"),
            destroy_config: symbol!(va, b"vaDestroyConfig\0"),
            create_surfaces: symbol!(va, b"vaCreateSurfaces\0"),
            destroy_surfaces: symbol!(va, b"vaDestroySurfaces\0"),
            sync_surface: symbol!(va, b"vaSyncSurface\0"),
            create_context: symbol!(va, b"vaCreateContext\0"),
            destroy_context: symbol!(va, b"vaDestroyContext\0"),
            create_buffer: symbol!(va, b"vaCreateBuffer\0"),
            destroy_buffer: symbol!(va, b"vaDestroyBuffer\0"),
            begin_picture: symbol!(va, b"vaBeginPicture\0"),
            render_picture: symbol!(va, b"vaRenderPicture\0"),
            end_picture: symbol!(va, b"vaEndPicture\0"),
            export_surface: symbol!(va, b"vaExportSurfaceHandle\0"),
            derive_image: symbol!(va, b"vaDeriveImage\0"),
            destroy_image: symbol!(va, b"vaDestroyImage\0"),
            map_buffer: symbol!(va, b"vaMapBuffer\0"),
            unmap_buffer: symbol!(va, b"vaUnmapBuffer\0"),
            query_filters: symbol!(va, b"vaQueryVideoProcFilters\0"),
            query_filter_caps: symbol!(va, b"vaQueryVideoProcFilterCaps\0"),
            query_pipeline_caps: symbol!(va, b"vaQueryVideoProcPipelineCaps\0"),
        };
        Ok(Self {
            symbols,
            _va: va,
            _drm: drm,
        })
    }

    fn text(&self, status: VAStatus) -> String {
        // SAFETY: `vaErrorStr` returns a static, non-null string for any value.
        let message = unsafe { CStr::from_ptr((self.symbols.error)(status)) };
        format!("{} ({status})", message.to_string_lossy())
    }

    fn check(&self, what: &str, status: VAStatus) -> Result<(), Error> {
        if status == VA_STATUS_SUCCESS {
            Ok(())
        } else {
            Err(Error::Unavailable(format!("{what}: {}", self.text(status))))
        }
    }
}

pub struct Display {
    library: Library,
    raw: VADisplay,
    fd: c_int,
    pub vendor: String,
    pub version: (c_int, c_int),
}

impl std::fmt::Debug for Display {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Display({})", self.vendor)
    }
}

// SAFETY: a `Display` is owned by one decoder and moved between threads whole.
unsafe impl Send for Display {}

impl Display {
    pub fn open(node: &str) -> Result<Self, Error> {
        let library = Library::load()?;
        let path = CString::new(node).map_err(|_| Error::Unsupported("a path with a nought"))?;
        // SAFETY: `path` is a valid C string; O_RDWR is 2 on Linux.
        let fd = unsafe { open(path.as_ptr(), 2) };
        if fd < 0 {
            return Err(Error::Unavailable(format!("{node} を開けません")));
        }
        // SAFETY: `fd` is an open DRM node; it is not taken over and is closed in `Drop`.
        let raw = unsafe { (library.symbols.get_display_drm)(fd) };
        if raw.is_null() {
            // SAFETY: `fd` came from `open` above and is closed once.
            unsafe { close(fd) };
            return Err(Error::Unavailable(String::from(
                "vaGetDisplayDRM が null を返しました",
            )));
        }
        let mut major = 0;
        let mut minor = 0;
        // SAFETY: `raw` was just made and the out parameters are live locals.
        let status = unsafe { (library.symbols.initialize)(raw, &mut major, &mut minor) };
        if status != VA_STATUS_SUCCESS {
            let message = library.text(status);
            // SAFETY: as above. 初期化に失敗した display も `vaTerminate` で解放する。
            unsafe {
                (library.symbols.terminate)(raw);
                close(fd);
            }
            return Err(Error::Unavailable(format!("vaInitialize: {message}")));
        }
        // SAFETY: the display is initialised; the string lives as long as the driver.
        let vendor = unsafe { CStr::from_ptr((library.symbols.vendor)(raw)) }
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            library,
            raw,
            fd,
            vendor,
            version: (major, minor),
        })
    }

    pub fn open_any() -> Result<Self, Error> {
        let mut last = Error::Unavailable(String::from("/dev/dri にレンダーノードがありません"));
        let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                name.starts_with("renderD")
                    .then(|| format!("/dev/dri/{name}"))
            })
            .collect();
        nodes.sort();
        for node in nodes {
            match Self::open(&node) {
                Ok(display) => return Ok(display),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    /// このプロファイルで使えるエントリーポイント。取得できなければ空。
    fn entrypoints(&self, profile: c_int) -> Vec<c_int> {
        // SAFETY: the display is initialised.
        let most = unsafe { (self.library.symbols.max_entrypoints)(self.raw) }.clamp(0, 32);
        let mut entrypoints = vec![0; most as usize];
        let mut count = 0;
        // SAFETY: `entrypoints` has room for `most` entries and `count` is live.
        let status = unsafe {
            (self.library.symbols.query_entrypoints)(
                self.raw,
                profile,
                entrypoints.as_mut_ptr(),
                &mut count,
            )
        };
        if status != VA_STATUS_SUCCESS {
            return Vec::new();
        }
        entrypoints.truncate(count.clamp(0, most) as usize);
        entrypoints
    }

    pub fn has(&self, profile: c_int) -> bool {
        self.entrypoints(profile).contains(&VA_ENTRYPOINT_VLD)
    }

    pub fn config(&self, profile: c_int, rt_format: c_uint) -> Result<VAConfigID, Error> {
        let mut attributes = [ConfigAttrib {
            kind: VA_CONFIG_ATTRIB_RT_FORMAT,
            value: 0,
        }];
        // SAFETY: one live element and that count is given.
        let status = unsafe {
            (self.library.symbols.get_config_attributes)(
                self.raw,
                profile,
                VA_ENTRYPOINT_VLD,
                attributes.as_mut_ptr(),
                1,
            )
        };
        self.library.check("vaGetConfigAttributes", status)?;
        if attributes[0].value & rt_format == 0 {
            return Err(Error::Unavailable(format!(
                "このプロファイルは 0x{rt_format:x} を出力できません（0x{:x}）",
                attributes[0].value
            )));
        }
        attributes[0].value = rt_format;
        let mut config = VA_INVALID_ID;
        // SAFETY: as above, with `config` a live local libva writes once.
        let status = unsafe {
            (self.library.symbols.create_config)(
                self.raw,
                profile,
                VA_ENTRYPOINT_VLD,
                attributes.as_mut_ptr(),
                1,
                &mut config,
            )
        };
        self.library.check("vaCreateConfig", status)?;
        Ok(config)
    }

    pub fn destroy_config(&self, config: VAConfigID) {
        // SAFETY: the config was made on this display and destroyed once.
        unsafe { (self.library.symbols.destroy_config)(self.raw, config) };
    }

    pub fn surfaces(
        &self,
        rt_format: c_uint,
        fourcc: u32,
        width: u32,
        height: u32,
        count: usize,
    ) -> Result<Vec<VASurfaceID>, Error> {
        let mut attributes = [SurfaceAttrib {
            kind: VA_SURFACE_ATTRIB_PIXEL_FORMAT,
            flags: VA_SURFACE_ATTRIB_SETTABLE,
            value: GenericValue {
                kind: VA_GENERIC_VALUE_TYPE_INTEGER,
                value: u64::from(fourcc),
            },
        }];
        let mut surfaces = vec![VA_INVALID_SURFACE; count];
        // SAFETY: both arrays outlive the call and hold what the counts say.
        let status = unsafe {
            (self.library.symbols.create_surfaces)(
                self.raw,
                rt_format,
                width,
                height,
                surfaces.as_mut_ptr(),
                count as c_uint,
                attributes.as_mut_ptr(),
                1,
            )
        };
        self.library.check("vaCreateSurfaces", status)?;
        Ok(surfaces)
    }

    pub fn raw(&self) -> VADisplay {
        self.raw
    }

    pub fn import_nv12(
        &self,
        fd: c_int,
        bytes: u32,
        size: [u32; 2],
        pitch: u32,
        chroma_offset: u32,
    ) -> Result<VASurfaceID, Error> {
        let mut descriptor = PrimeDescriptor {
            fourcc: VA_FOURCC_NV12,
            width: size[0],
            height: size[1],
            num_objects: 1,
            num_layers: 1,
            ..Default::default()
        };
        descriptor.objects[0] = PrimeObject {
            fd,
            size: bytes,
            drm_format_modifier: 0,
        };
        descriptor.layers[0] = PrimeLayer {
            drm_format: VA_FOURCC_NV12,
            num_planes: 2,
            object_index: [0; 4],
            offset: [0, chroma_offset, 0, 0],
            pitch: [pitch, pitch, 0, 0],
        };
        let integer = |kind, value: u64| SurfaceAttrib {
            kind,
            flags: VA_SURFACE_ATTRIB_SETTABLE,
            value: GenericValue {
                kind: VA_GENERIC_VALUE_TYPE_INTEGER,
                value,
            },
        };
        let mut attributes = [
            integer(VA_SURFACE_ATTRIB_PIXEL_FORMAT, u64::from(VA_FOURCC_NV12)),
            integer(
                VA_SURFACE_ATTRIB_MEMORY_TYPE,
                u64::from(VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2),
            ),
            SurfaceAttrib {
                kind: VA_SURFACE_ATTRIB_EXTERNAL_BUFFER_DESCRIPTOR,
                flags: VA_SURFACE_ATTRIB_SETTABLE,
                value: GenericValue {
                    kind: VA_GENERIC_VALUE_TYPE_POINTER,
                    value: std::ptr::from_mut(&mut descriptor) as u64,
                },
            },
        ];
        let mut surface = VA_INVALID_SURFACE;
        // SAFETY: 属性と記述子は呼び出しの間生きるローカル変数。
        let status = unsafe {
            (self.library.symbols.create_surfaces)(
                self.raw,
                VA_RT_FORMAT_YUV420,
                size[0],
                size[1],
                &mut surface,
                1,
                attributes.as_mut_ptr(),
                attributes.len() as c_uint,
            )
        };
        self.library.check("vaCreateSurfaces（DMA-BUF）", status)?;
        Ok(surface)
    }

    pub fn destroy_surfaces(&self, surfaces: &mut [VASurfaceID]) {
        if surfaces.is_empty() {
            return;
        }
        // SAFETY: the slice is non-empty and its length is what is given.
        unsafe {
            (self.library.symbols.destroy_surfaces)(
                self.raw,
                surfaces.as_mut_ptr(),
                surfaces.len() as c_int,
            )
        };
    }

    pub fn has_video_proc(&self) -> bool {
        self.entrypoints(VA_PROFILE_NONE)
            .contains(&VA_ENTRYPOINT_VIDEO_PROC)
    }

    pub fn video_proc_config(&self) -> Result<VAConfigID, Error> {
        let mut config = VA_INVALID_ID;
        // SAFETY: 属性は無し（null と 0）、`config` は libva が一度だけ書くローカル変数。
        let status = unsafe {
            (self.library.symbols.create_config)(
                self.raw,
                VA_PROFILE_NONE,
                VA_ENTRYPOINT_VIDEO_PROC,
                std::ptr::null_mut(),
                0,
                &mut config,
            )
        };
        self.library.check("vaCreateConfig(VideoProc)", status)?;
        Ok(config)
    }

    pub fn deinterlacers(&self, context: VAContextID) -> Vec<c_int> {
        let mut filters = [0 as c_int; VA_PROC_FILTER_COUNT];
        let mut count = VA_PROC_FILTER_COUNT as c_uint;
        // SAFETY: 配列は `count` 個あり、ドライバーは書いた数を返す。
        let status = unsafe {
            (self.library.symbols.query_filters)(
                self.raw,
                context,
                filters.as_mut_ptr(),
                &mut count,
            )
        };
        if status != VA_STATUS_SUCCESS
            || !filters[..(count as usize).min(VA_PROC_FILTER_COUNT)]
                .contains(&VA_PROC_FILTER_DEINTERLACING)
        {
            return Vec::new();
        }
        let mut caps = [DeinterlacingCap::default(); VA_PROC_DEINTERLACING_COUNT];
        let mut count = VA_PROC_DEINTERLACING_COUNT as c_uint;
        // SAFETY: 配列は `count` 個の `VAProcFilterCapDeinterlacing`。
        let status = unsafe {
            (self.library.symbols.query_filter_caps)(
                self.raw,
                context,
                VA_PROC_FILTER_DEINTERLACING,
                caps.as_mut_ptr().cast(),
                &mut count,
            )
        };
        if status != VA_STATUS_SUCCESS {
            return Vec::new();
        }
        caps[..(count as usize).min(VA_PROC_DEINTERLACING_COUNT)]
            .iter()
            .map(|cap| cap.kind)
            .collect()
    }

    pub fn references(
        &self,
        context: VAContextID,
        filter: VABufferID,
    ) -> Result<(u32, u32), Error> {
        let mut caps = PipelineCaps::default();
        let mut filters = [filter];
        // SAFETY: フィルターのバッファは一つで、その数を渡す。`caps` はドライバーが書くローカル変数。
        let status = unsafe {
            (self.library.symbols.query_pipeline_caps)(
                self.raw,
                context,
                filters.as_mut_ptr(),
                1,
                &mut caps,
            )
        };
        self.library.check("vaQueryVideoProcPipelineCaps", status)?;
        Ok((caps.num_forward_references, caps.num_backward_references))
    }

    pub fn destroy_buffer(&self, buffer: VABufferID) {
        // SAFETY: この display の上で作ったバッファで、解放するのは一度だけ。
        unsafe { (self.library.symbols.destroy_buffer)(self.raw, buffer) };
    }

    pub fn process(
        &self,
        context: VAContextID,
        into: VASurfaceID,
        buffers: &mut Vec<VABufferID>,
    ) -> Result<(), Error> {
        self.decode(context, into, buffers)
    }

    pub fn context(
        &self,
        config: VAConfigID,
        width: u32,
        height: u32,
        surfaces: &mut [VASurfaceID],
    ) -> Result<VAContextID, Error> {
        let mut context = VA_INVALID_ID;
        // SAFETY: the surfaces came from this display; `context` is a live local.
        let status = unsafe {
            (self.library.symbols.create_context)(
                self.raw,
                config,
                width as c_int,
                height as c_int,
                0,
                surfaces.as_mut_ptr(),
                surfaces.len() as c_int,
                &mut context,
            )
        };
        self.library.check("vaCreateContext", status)?;
        Ok(context)
    }

    pub fn destroy_context(&self, context: VAContextID) {
        // SAFETY: the context was made on this display and destroyed once.
        unsafe { (self.library.symbols.destroy_context)(self.raw, context) };
    }

    fn buffer(
        &self,
        context: VAContextID,
        kind: c_int,
        size: usize,
        data: *const c_void,
    ) -> Result<VABufferID, Error> {
        let mut buffer = VA_INVALID_ID;
        // SAFETY: `data` points at `size` readable bytes for the call.
        let status = unsafe {
            (self.library.symbols.create_buffer)(
                self.raw,
                context,
                kind,
                size as c_uint,
                1,
                data,
                &mut buffer,
            )
        };
        self.library.check("vaCreateBuffer", status)?;
        Ok(buffer)
    }

    pub fn parameters<T>(
        &self,
        context: VAContextID,
        kind: c_int,
        value: &T,
    ) -> Result<VABufferID, Error> {
        self.buffer(context, kind, size_of::<T>(), (value as *const T).cast())
    }

    pub fn coded(&self, context: VAContextID, data: &[u8]) -> Result<VABufferID, Error> {
        self.buffer(
            context,
            VA_SLICE_DATA_BUFFER_TYPE,
            data.len().max(1),
            data.as_ptr().cast(),
        )
    }

    /// Buffers are destroyed whatever happens, or the driver soon runs out.
    pub fn decode(
        &self,
        context: VAContextID,
        into: VASurfaceID,
        buffers: &mut Vec<VABufferID>,
    ) -> Result<(), Error> {
        let result = self.submit(context, into, buffers);
        for buffer in buffers.drain(..) {
            self.destroy_buffer(buffer);
        }
        result
    }

    fn submit(
        &self,
        context: VAContextID,
        into: VASurfaceID,
        buffers: &mut [VABufferID],
    ) -> Result<(), Error> {
        // SAFETY: `into` is a surface of the pool this context was made with.
        let status = unsafe { (self.library.symbols.begin_picture)(self.raw, context, into) };
        self.library.check("vaBeginPicture", status)?;
        // SAFETY: the slice is live; buffers are destroyed after `vaEndPicture`.
        let status = unsafe {
            (self.library.symbols.render_picture)(
                self.raw,
                context,
                buffers.as_mut_ptr(),
                buffers.len() as c_int,
            )
        };
        let rendered = self.library.check("vaRenderPicture", status);
        // SAFETY: a begun picture must be ended even if the render failed.
        let status = unsafe { (self.library.symbols.end_picture)(self.raw, context) };
        rendered?;
        self.library.check("vaEndPicture", status)
    }

    pub fn sync(&self, surface: VASurfaceID) -> Result<(), Error> {
        // SAFETY: `surface` came from `vaCreateSurfaces` on this display.
        let status = unsafe { (self.library.symbols.sync_surface)(self.raw, surface) };
        self.library.check("vaSyncSurface", status)
    }

    pub fn export(&self, surface: VASurfaceID) -> Result<PrimeDescriptor, Error> {
        let mut descriptor = PrimeDescriptor::default();
        // SAFETY: a live local of exactly the type libva writes for `DRM_PRIME_2`.
        let status = unsafe {
            (self.library.symbols.export_surface)(
                self.raw,
                surface,
                VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
                VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_SEPARATE_LAYERS,
                (&raw mut descriptor).cast(),
            )
        };
        self.library.check("vaExportSurfaceHandle", status)?;
        Ok(descriptor)
    }

    pub fn read_back(&self, surface: VASurfaceID) -> Result<Readback, Error> {
        self.sync(surface)?;
        // `vaDeriveImage` に対応しないドライバーでは、エクスポートした DMA-BUF を `mmap` して読む。
        match self.read_back_derived(surface) {
            Ok(read) => Ok(read),
            Err(derived) => self.read_back_exported(surface).map_err(|exported| {
                Error::Unavailable(format!("{derived:?} と、エクスポートからも {exported:?}"))
            }),
        }
    }

    fn read_back_exported(&self, surface: VASurfaceID) -> Result<Readback, Error> {
        let descriptor = self.export(surface)?;
        let height = descriptor.height as usize;
        let copied = (|| -> Result<Vec<(Vec<u8>, usize, usize)>, Error> {
            let mut planes = Vec::new();
            for layer in 0..descriptor.num_layers.min(3) as usize {
                let layer = &descriptor.layers[layer];
                for plane in 0..layer.num_planes.min(4) as usize {
                    let which = layer.object_index[plane] as usize;
                    let object = descriptor
                        .objects
                        .get(which)
                        .ok_or(Error::Unsupported("a layer naming no object"))?;
                    // 線形でなければ読めない。エラーにしないと壊れたフレームを返す。
                    if object.drm_format_modifier != DRM_FORMAT_MOD_LINEAR {
                        return Err(Error::Unsupported("an exported surface that is not linear"));
                    }
                    let pitch = layer.pitch[plane] as usize;
                    let offset = layer.offset[plane] as usize;
                    let rows = if planes.is_empty() {
                        height
                    } else {
                        height.div_ceil(2)
                    };
                    if pitch == 0 || offset + rows * pitch > object.size as usize {
                        return Err(Error::Unsupported("a layer outside its object"));
                    }
                    let mapping =
                        Mapping::new(object.fd, 0, object.size as usize, false).map_err(|_| {
                            Error::Unavailable(String::from("DMA-BUF を mmap できません"))
                        })?;
                    // 範囲は上で確かめてある。
                    let out = mapping.bytes()[offset..offset + rows * pitch].to_vec();
                    planes.push((out, pitch, rows));
                }
            }
            Ok(planes)
        })();
        for object in 0..descriptor.num_objects.min(4) as usize {
            // SAFETY: エクスポートが所有権ごと渡したファイル記述子で、閉じるのは一度。
            unsafe { close(descriptor.objects[object].fd) };
        }
        let planes = copied?;
        if planes.is_empty() {
            return Err(Error::Unsupported("an export with no layers"));
        }
        Ok(Readback {
            fourcc: descriptor.fourcc,
            width: descriptor.width as usize,
            height,
            planes,
        })
    }

    fn read_back_derived(&self, surface: VASurfaceID) -> Result<Readback, Error> {
        let mut image = Image::default();
        // SAFETY: `image` is a live `VAImage` with the reserved words included.
        let status = unsafe { (self.library.symbols.derive_image)(self.raw, surface, &mut image) };
        self.library.check("vaDeriveImage", status)?;
        let mut base: *mut c_void = std::ptr::null_mut();
        // SAFETY: `image.buf` is the buffer libva just made for this image.
        let status = unsafe { (self.library.symbols.map_buffer)(self.raw, image.buf, &mut base) };
        if status != VA_STATUS_SUCCESS {
            // SAFETY: the image was derived and is destroyed exactly once.
            unsafe { (self.library.symbols.destroy_image)(self.raw, image.image_id) };
            return Err(Error::Unavailable(format!(
                "vaMapBuffer: {}",
                self.library.text(status)
            )));
        }
        let width = image.width as usize;
        let height = image.height as usize;
        let planes = (0..image.num_planes.min(3) as usize)
            .map(|plane| {
                let rows = if plane == 0 {
                    height
                } else {
                    height.div_ceil(2)
                };
                let pitch = image.pitches[plane] as usize;
                let mut out = vec![0u8; pitch * rows];
                for y in 0..rows {
                    let at = image.offsets[plane] as usize + y * pitch;
                    // SAFETY: `at + pitch` is inside the mapping; the slice drops before unmapping.
                    let row =
                        unsafe { std::slice::from_raw_parts(base.cast::<u8>().add(at), pitch) };
                    out[y * pitch..(y + 1) * pitch].copy_from_slice(row);
                }
                (out, pitch, rows)
            })
            .collect::<Vec<_>>();
        // SAFETY: mapped above, slices dropped; the image is destroyed once.
        unsafe {
            (self.library.symbols.unmap_buffer)(self.raw, image.buf);
            (self.library.symbols.destroy_image)(self.raw, image.image_id);
        }
        Ok(Readback {
            fourcc: image.format.fourcc,
            width,
            height,
            planes,
        })
    }
}

#[derive(Debug)]
pub struct Readback {
    pub fourcc: u32,
    pub width: usize,
    pub height: usize,
    pub planes: Vec<(Vec<u8>, usize, usize)>,
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: initialised in `open`; terminated and closed once, in that order.
        unsafe {
            (self.library.symbols.terminate)(self.raw);
            close(self.fd);
        }
    }
}

pub fn duplicate(fd: c_int) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: `fd` is still owned by libva; `dup` makes one this process owns.
    let copy = unsafe { dup(fd) };
    if copy < 0 {
        return None;
    }
    // SAFETY: `copy` is a fresh descriptor owned by nobody else.
    Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(copy) })
}

pub fn discard(fd: c_int) {
    // SAFETY: `vaExportSurfaceHandle` passes ownership; closed once.
    unsafe { close(fd) };
}

unsafe extern "C" {
    // C では可変長引数。固定の引数では aarch64 の rustc が標準ライブラリの `open` と宣言が合わないとエラーにする
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn dup(fd: c_int) -> c_int;
}

pub fn fourcc(value: u32) -> String {
    value.to_le_bytes().iter().map(|b| *b as char).collect()
}

pub struct Buffers<'a> {
    display: &'a Display,
    ids: Vec<VABufferID>,
}

impl<'a> Buffers<'a> {
    pub fn new(display: &'a Display) -> Self {
        Self {
            display,
            ids: Vec::new(),
        }
    }
}

impl std::ops::Deref for Buffers<'_> {
    type Target = Vec<VABufferID>;

    fn deref(&self) -> &Self::Target {
        &self.ids
    }
}

impl std::ops::DerefMut for Buffers<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ids
    }
}

impl Drop for Buffers<'_> {
    fn drop(&mut self) {
        for buffer in self.ids.drain(..) {
            self.display.destroy_buffer(buffer);
        }
    }
}

impl std::fmt::Debug for Buffers<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "Buffers({})", self.ids.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_structs_are_the_size_libva_declares() {
        assert_eq!(size_of::<ImageFormat>(), 48);
        assert_eq!(size_of::<Image>(), 120);
        assert_eq!(size_of::<GenericValue>(), 16);
        assert_eq!(size_of::<SurfaceAttrib>(), 24);
        assert_eq!(size_of::<PrimeObject>(), 16);
        assert_eq!(size_of::<PrimeLayer>(), 56);
        assert_eq!(size_of::<PrimeDescriptor>(), 312);
        assert_eq!(size_of::<DeinterlacingParameters>(), 28);
        assert_eq!(size_of::<DeinterlacingCap>(), 20);
        assert_eq!(size_of::<ColorProperties>(), 8);
        assert_eq!(size_of::<PipelineParameters>(), 224);
        assert_eq!(
            std::mem::offset_of!(PipelineParameters, input_surface_flag),
            124
        );
        assert_eq!(
            std::mem::offset_of!(PipelineParameters, output_hdr_metadata),
            152
        );
        assert_eq!(size_of::<PipelineCaps>(), 176);
    }

    #[test]
    fn the_prime_memory_type_has_the_digits_it_has() {
        assert_eq!(VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2, 1 << 30);
    }

    #[test]
    fn a_fourcc_reads_the_way_it_is_written() {
        assert_eq!(fourcc(VA_FOURCC_NV12), "NV12");
        assert_eq!(fourcc(VA_FOURCC_P010), "P010");
    }
}
