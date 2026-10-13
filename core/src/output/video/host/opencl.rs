// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Intel のハードウェア動き検出（VME）を OpenCL で使う：前と今のフレーム間のオプティカルフロー（補間方式の名前は `intel`）。
//! VME を持つのは Gen9 と Ice Lake だけ（Gen12 以降は [`super::compute`]）。
//! カーネルは ±16×12 しか探索しないので、1/4 縮小で大まかに探索し、その予測で等倍を精密化する。

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

use super::{Convert, Gpu, Run};

#[cfg(all(feature = "vaapi", target_os = "linux"))]
mod va;

type Int = i32;
type Handle = *mut c_void;

const SUCCESS: Int = 0;
const DEVICE_TYPE_GPU: u64 = 1 << 2;
const DEVICE_NAME: u32 = 0x102B;
const DEVICE_EXTENSIONS: u32 = 0x1030;
const MEM_WRITE_ONLY: u64 = 1 << 1;
const MEM_READ_ONLY: u64 = 1 << 2;
/// メインメモリ側に置く（内蔵 GPU では map がコピー無しになり、読み出しも GPU のコピーを待たない）。
const MEM_ALLOC_HOST_PTR: u64 = 1 << 4;
const MAP_READ: u64 = 1;
const MAP_WRITE_INVALIDATE_REGION: u64 = 1 << 2;
const CHANNEL_R: u32 = 0x10B0;
const UNORM_INT8: u32 = 0x10D2;
const MEM_OBJECT_IMAGE2D: u32 = 0x10F1;
const TRUE: u32 = 1;
const FALSE: u32 = 0;
// `cl_intel_motion_estimation`。
const ACCELERATOR_MOTION_ESTIMATION: u32 = 0;
const MB_TYPE_4X4: u32 = 2;
const SUBPIXEL_QPEL: u32 = 2;
const SAD_ADJUST_NONE: u32 = 0;
const SEARCH_RADIUS_4_4: u32 = 1;
const SEARCH_RADIUS_16_12: u32 = 5;
const KERNEL: &CStr = c"block_motion_estimate_intel";
const EXTENSION: &str = "cl_intel_motion_estimation";

#[repr(C)]
struct ImageFormat {
    order: u32,
    kind: u32,
}

#[repr(C)]
struct ImageDesc {
    kind: u32,
    width: usize,
    height: usize,
    depth: usize,
    array_size: usize,
    row_pitch: usize,
    slice_pitch: usize,
    mip_levels: u32,
    samples: u32,
    buffer: Handle,
}

#[repr(C)]
struct MotionDesc {
    block: u32,
    subpixel: u32,
    sad_adjust: u32,
    search: u32,
}

#[derive(Clone, Copy)]
struct Api {
    platforms: unsafe extern "system" fn(u32, *mut Handle, *mut u32) -> Int,
    devices: unsafe extern "system" fn(Handle, u64, u32, *mut Handle, *mut u32) -> Int,
    device_info: unsafe extern "system" fn(Handle, u32, usize, *mut c_void, *mut usize) -> Int,
    create_context: unsafe extern "system" fn(
        *const isize,
        u32,
        *const Handle,
        *mut c_void,
        *mut c_void,
        *mut Int,
    ) -> Handle,
    create_queue: unsafe extern "system" fn(Handle, Handle, u64, *mut Int) -> Handle,
    create_image: unsafe extern "system" fn(
        Handle,
        u64,
        *const ImageFormat,
        *const ImageDesc,
        *mut c_void,
        *mut Int,
    ) -> Handle,
    create_buffer: unsafe extern "system" fn(Handle, u64, usize, *mut c_void, *mut Int) -> Handle,
    builtin_program:
        unsafe extern "system" fn(Handle, u32, *const Handle, *const c_char, *mut Int) -> Handle,
    create_kernel: unsafe extern "system" fn(Handle, *const c_char, *mut Int) -> Handle,
    set_arg: unsafe extern "system" fn(Handle, u32, usize, *const c_void) -> Int,
    write_buffer: unsafe extern "system" fn(
        Handle,
        Handle,
        u32,
        usize,
        usize,
        *const c_void,
        u32,
        *const Handle,
        *mut Handle,
    ) -> Int,
    copy_to_image: unsafe extern "system" fn(
        Handle,
        Handle,
        Handle,
        usize,
        *const usize,
        *const usize,
        u32,
        *const Handle,
        *mut Handle,
    ) -> Int,
    #[cfg_attr(not(all(feature = "vaapi", target_os = "linux")), allow(dead_code))]
    copy_image: unsafe extern "system" fn(
        Handle,
        Handle,
        Handle,
        *const usize,
        *const usize,
        *const usize,
        u32,
        *const Handle,
        *mut Handle,
    ) -> Int,
    run: unsafe extern "system" fn(
        Handle,
        Handle,
        u32,
        *const usize,
        *const usize,
        *const usize,
        u32,
        *const Handle,
        *mut Handle,
    ) -> Int,
    map_buffer: unsafe extern "system" fn(
        Handle,
        Handle,
        u32,
        u64,
        usize,
        usize,
        u32,
        *const Handle,
        *mut Handle,
        *mut Int,
    ) -> *mut c_void,
    unmap: unsafe extern "system" fn(
        Handle,
        Handle,
        *mut c_void,
        u32,
        *const Handle,
        *mut Handle,
    ) -> Int,
    finish: unsafe extern "system" fn(Handle) -> Int,
    extension: unsafe extern "system" fn(Handle, *const c_char) -> *mut c_void,
    release_mem: unsafe extern "system" fn(Handle) -> Int,
    release_kernel: unsafe extern "system" fn(Handle) -> Int,
    release_program: unsafe extern "system" fn(Handle) -> Int,
    release_queue: unsafe extern "system" fn(Handle) -> Int,
    release_context: unsafe extern "system" fn(Handle) -> Int,
}

fn api() -> Result<&'static Api, String> {
    static OPENED: OnceLock<Result<(crate::dl::Object, Api), String>> = OnceLock::new();
    OPENED
        .get_or_init(|| {
            let name = if cfg!(windows) {
                "OpenCL.dll"
            } else {
                "libOpenCL.so.1"
            };
            let object = crate::dl::Object::open(name).map_err(|e| e.0)?;
            let missing = |e: crate::dl::Missing| e.0;
            // SAFETY: シンボルの型は `CL/cl.h` の宣言のとおり。
            let api = unsafe {
                Api {
                    platforms: object.symbol(b"clGetPlatformIDs\0").map_err(missing)?,
                    devices: object.symbol(b"clGetDeviceIDs\0").map_err(missing)?,
                    device_info: object.symbol(b"clGetDeviceInfo\0").map_err(missing)?,
                    create_context: object.symbol(b"clCreateContext\0").map_err(missing)?,
                    create_queue: object.symbol(b"clCreateCommandQueue\0").map_err(missing)?,
                    create_image: object.symbol(b"clCreateImage\0").map_err(missing)?,
                    create_buffer: object.symbol(b"clCreateBuffer\0").map_err(missing)?,
                    builtin_program: object
                        .symbol(b"clCreateProgramWithBuiltInKernels\0")
                        .map_err(missing)?,
                    create_kernel: object.symbol(b"clCreateKernel\0").map_err(missing)?,
                    set_arg: object.symbol(b"clSetKernelArg\0").map_err(missing)?,
                    write_buffer: object.symbol(b"clEnqueueWriteBuffer\0").map_err(missing)?,
                    copy_to_image: object
                        .symbol(b"clEnqueueCopyBufferToImage\0")
                        .map_err(missing)?,
                    copy_image: object.symbol(b"clEnqueueCopyImage\0").map_err(missing)?,
                    run: object
                        .symbol(b"clEnqueueNDRangeKernel\0")
                        .map_err(missing)?,
                    map_buffer: object.symbol(b"clEnqueueMapBuffer\0").map_err(missing)?,
                    unmap: object
                        .symbol(b"clEnqueueUnmapMemObject\0")
                        .map_err(missing)?,
                    finish: object.symbol(b"clFinish\0").map_err(missing)?,
                    extension: object
                        .symbol(b"clGetExtensionFunctionAddressForPlatform\0")
                        .map_err(missing)?,
                    release_mem: object.symbol(b"clReleaseMemObject\0").map_err(missing)?,
                    release_kernel: object.symbol(b"clReleaseKernel\0").map_err(missing)?,
                    release_program: object.symbol(b"clReleaseProgram\0").map_err(missing)?,
                    release_queue: object.symbol(b"clReleaseCommandQueue\0").map_err(missing)?,
                    release_context: object.symbol(b"clReleaseContext\0").map_err(missing)?,
                }
            };
            Ok((object, api))
        })
        .as_ref()
        .map(|(_, api)| api)
        .map_err(Clone::clone)
}

fn find_device(api: &Api) -> Result<(Handle, Handle, String), String> {
    let mut platforms = [std::ptr::null_mut(); 8];
    let mut count = 0u32;
    // SAFETY: 大きさを指定したローカル変数。
    if unsafe { (api.platforms)(8, platforms.as_mut_ptr(), &mut count) } != SUCCESS {
        return Err(String::from("OpenCL のプラットフォームが無い"));
    }
    let text = |device: Handle, what: u32| -> String {
        let mut size = 0usize;
        // SAFETY: 大きさだけ先に問い合わせる。
        unsafe { (api.device_info)(device, what, 0, std::ptr::null_mut(), &mut size) };
        let mut buffer = vec![0u8; size.max(1)];
        // SAFETY: 問い合わせた大きさのバッファ。
        unsafe {
            (api.device_info)(
                device,
                what,
                buffer.len(),
                buffer.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            )
        };
        String::from_utf8_lossy(&buffer)
            .trim_end_matches('\0')
            .to_owned()
    };
    for &platform in &platforms[..(count as usize).min(8)] {
        let mut devices = [std::ptr::null_mut(); 4];
        let mut found = 0u32;
        // SAFETY: 同上。
        if unsafe {
            (api.devices)(
                platform,
                DEVICE_TYPE_GPU,
                4,
                devices.as_mut_ptr(),
                &mut found,
            )
        } != SUCCESS
        {
            continue;
        }
        for &device in &devices[..(found as usize).min(4)] {
            if text(device, DEVICE_EXTENSIONS)
                .split_whitespace()
                .any(|e| e == EXTENSION)
            {
                return Ok((platform, device, text(device, DEVICE_NAME)));
            }
        }
    }
    Err(format!(
        "{EXTENSION} を持つ GPU が無い（VME は Gen9 と Ice Lake だけ）"
    ))
}

pub fn supported() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| api().is_ok_and(|api| find_device(api).is_ok()))
}

fn check(what: &str, code: Int) -> Result<(), String> {
    if code == SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}：{code}"))
    }
}

struct Level {
    size: [usize; 2],
    blocks: [usize; 2],
    /// 2 つのイメージを交互に使う（今は `Flow::newest`）。新しいフレームは前々のフレームの所へだけ書く。
    images: [Handle; 2],
    /// メインメモリから書くバッファ。イメージへは GPU でコピーする（`clEnqueueWriteImage` は遅い）。
    /// バッファから作ったイメージは VME がクラッシュするので使わない。
    staging: Handle,
    /// 方向ごとの結果。等倍は予測ありの結果で、予測なしは `plain_*`（誤差の小さい方を採る）。
    vectors: [Handle; 2],
    residuals: [Handle; 2],
    plain_vectors: [Handle; 2],
    plain_residuals: [Handle; 2],
}

pub struct Flow {
    api: Api,
    context: Handle,
    queue: Handle,
    program: Handle,
    /// アクセラレーター：`[広く（±16×12）, 狭く（±4×4）]`。予測のある等倍の探索は狭く。
    accelerators: [Handle; 2],
    /// アクセラレーターの設定（作るときに渡したアドレスのまま持つ）。
    _descs: [Box<MotionDesc>; 2],
    release_accelerator: Option<unsafe extern "system" fn(Handle) -> Int>,
    size: [u32; 2],
    wanted: u32,
    fine: Level,
    coarse: Level,
    predictions: [Handle; 2],
    luma: Luma,
    /// NVOFA と同じ形式の生のフロー（`Rg16Sint`、グリッド 4）とコスト（`R8Uint`）。
    raw: [wgpu::Texture; 2],
    costs: [wgpu::Texture; 2],
    has_prev: bool,
    newest: usize,
    outside: f64,
    #[cfg(all(feature = "vaapi", target_os = "linux"))]
    va: Option<va::Share>,
}

struct Luma {
    pipeline: wgpu::ComputePipeline,
    sampler: wgpu::Sampler,
    written: [wgpu::Buffer; 2],
    readback: [wgpu::Buffer; 2],
    params: [wgpu::Buffer; 2],
    sizes: [[u32; 2]; 2],
}

impl Luma {
    fn new(device: &wgpu::Device, sizes: [[u32; 2]; 2], pictures: [[u32; 2]; 2]) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw vme luma"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vme.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("zw vme luma"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("zw vme luma"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let buffer = |label, bytes: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage,
                mapped_at_creation: false,
            })
        };
        let bytes = sizes.map(|s| u64::from(s[0] * s[1]));
        let written = bytes.map(|b| {
            buffer(
                "zw vme luma",
                b,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            )
        });
        let readback = bytes.map(|b| {
            buffer(
                "zw vme readback",
                b,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            )
        });
        let params = std::array::from_fn(|k| {
            let params = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zw vme luma params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: true,
            });
            let words = [sizes[k][0], sizes[k][1], pictures[k][0], pictures[k][1]];
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            if let Ok(mut range) = params.slice(..).get_mapped_range_mut() {
                range.copy_from_slice(&bytes);
            }
            params.unmap();
            params
        });
        Self {
            pipeline,
            sampler,
            written,
            readback,
            params,
            sizes,
        }
    }

    fn encode(&self, gpu: &mut Gpu<'_>, source: &wgpu::Texture, readback: bool) {
        let view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let layout = self.pipeline.get_bind_group_layout(0);
        let groups: Vec<wgpu::BindGroup> = (0..2)
            .map(|k| {
                gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("zw vme luma"),
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.written[k].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: self.params[k].as_entire_binding(),
                        },
                    ],
                })
            })
            .collect();
        {
            let mut pass = gpu
                .encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("zw vme luma"),
                    timestamp_writes: None,
                });
            pass.set_pipeline(&self.pipeline);
            for (k, group) in groups.iter().enumerate() {
                let [w, h] = self.sizes[k];
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups((w / 4).div_ceil(16), h.div_ceil(4), 1);
            }
        }
        if readback {
            for k in 0..2 {
                gpu.encoder
                    .copy_buffer_to_buffer(&self.written[k], 0, &self.readback[k], 0, None);
            }
        }
    }
}

struct Mapped<'a, T> {
    flow: &'a Flow,
    buffer: Handle,
    pointer: *mut T,
    count: usize,
}

impl<T> std::ops::Deref for Mapped<'_, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        // SAFETY: map したアドレスと大きさ。unmap するまで有効。
        unsafe { std::slice::from_raw_parts(self.pointer, self.count) }
    }
}

impl<T> std::ops::DerefMut for Mapped<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: 同上。この `Mapped` だけが持つ。
        unsafe { std::slice::from_raw_parts_mut(self.pointer, self.count) }
    }
}

impl<T> Drop for Mapped<'_, T> {
    fn drop(&mut self) {
        // SAFETY: 有効なキューと、map したバッファとアドレスを一度だけ。
        unsafe {
            (self.flow.api.unmap)(
                self.flow.queue,
                self.buffer,
                self.pointer.cast(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            );
        }
    }
}

const MEDIAN9: [(usize, usize); 19] = [
    (1, 2),
    (4, 5),
    (7, 8),
    (0, 1),
    (3, 4),
    (6, 7),
    (1, 2),
    (4, 5),
    (7, 8),
    (0, 3),
    (5, 8),
    (4, 7),
    (3, 6),
    (1, 4),
    (2, 5),
    (4, 7),
    (4, 2),
    (6, 4),
    (4, 2),
];

/// 9 本の配列の要素ごとの中央値を `lanes[4]` へ（ほかの配列は壊れる）。
fn median9_lanes(lanes: &mut [Vec<i16>; 9]) {
    for (a, b) in MEDIAN9 {
        let (p, q) = if a < b {
            let (first, second) = lanes.split_at_mut(b);
            (&mut first[a], &mut second[0])
        } else {
            let (first, second) = lanes.split_at_mut(a);
            (&mut second[0], &mut first[b])
        };
        for (p, q) in p.iter_mut().zip(q.iter_mut()) {
            let (low, high) = ((*p).min(*q), (*p).max(*q));
            *p = low;
            *q = high;
        }
    }
}

fn trace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZW_TRACE").is_some())
}

impl Flow {
    pub fn open(device: &wgpu::Device, size: [u32; 2], wanted: u32) -> Result<Self, String> {
        let api = *api()?;
        let (platform, cl_device, name) = find_device(&api)?;
        let mut code = SUCCESS;
        let plain = |code: &mut Int| {
            // SAFETY: 見つけたデバイスと有効なローカル変数。
            unsafe {
                (api.create_context)(
                    std::ptr::null(),
                    1,
                    &cl_device,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    code,
                )
            }
        };
        // レンダリング用 GPU が同じ Intel なら、VA-API の VADisplay を指定したコンテキスト（サーフェスをコピーなしで受ける）。
        #[cfg(all(feature = "vaapi", target_os = "linux"))]
        let (context, mut sharing) = match va::Opened::new(device) {
            Ok(opened) => {
                let properties = opened.properties(platform);
                // SAFETY: 0 で終わる属性の配列と、見つけたデバイス。
                let context = unsafe {
                    (api.create_context)(
                        properties.as_ptr(),
                        1,
                        &cl_device,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut code,
                    )
                };
                if code == SUCCESS {
                    (context, Ok(opened))
                } else {
                    (
                        plain(&mut code),
                        Err(format!("clCreateContext（VA-API）：{code}")),
                    )
                }
            }
            Err(why) => (plain(&mut code), Err(why)),
        };
        #[cfg(not(all(feature = "vaapi", target_os = "linux")))]
        let context = plain(&mut code);
        check("clCreateContext", code)?;
        // SAFETY: 同上。
        let queue = unsafe { (api.create_queue)(context, cl_device, 0, &mut code) };
        check("clCreateCommandQueue", code)?;
        // SAFETY: 名前は NUL 終端。
        let create_accelerator =
            unsafe { (api.extension)(platform, c"clCreateAcceleratorINTEL".as_ptr()) };
        let release_accelerator =
            unsafe { (api.extension)(platform, c"clReleaseAcceleratorINTEL".as_ptr()) };
        if create_accelerator.is_null() {
            return Err(String::from("clCreateAcceleratorINTEL が無い"));
        }
        // SAFETY: `clCreateAcceleratorINTEL_fn` の署名。
        let create_accelerator: unsafe extern "system" fn(
            Handle,
            u32,
            usize,
            *const c_void,
            *mut Int,
        ) -> Handle = unsafe { std::mem::transmute(create_accelerator) };
        // 設定は `Flow` が最後まで持つ（コピーせずにアドレスを保持している可能性がある）。
        let descs = [SEARCH_RADIUS_16_12, SEARCH_RADIUS_4_4].map(|search| {
            Box::new(MotionDesc {
                block: MB_TYPE_4X4,
                subpixel: SUBPIXEL_QPEL,
                sad_adjust: SAD_ADJUST_NONE,
                search,
            })
        });
        let mut accelerators = [std::ptr::null_mut(); 2];
        for (accelerator, desc) in accelerators.iter_mut().zip(&descs) {
            // SAFETY: 有効なコンテキストと記述子。
            *accelerator = unsafe {
                create_accelerator(
                    context,
                    ACCELERATOR_MOTION_ESTIMATION,
                    size_of::<MotionDesc>(),
                    std::ptr::from_ref(&**desc).cast(),
                    &mut code,
                )
            };
            check("clCreateAcceleratorINTEL", code)?;
        }
        // SAFETY: 有効なコンテキストとデバイス、NUL 終端の名前。
        let program =
            unsafe { (api.builtin_program)(context, 1, &cl_device, KERNEL.as_ptr(), &mut code) };
        check("clCreateProgramWithBuiltInKernels", code)?;

        let fine_size = [
            (size[0] as usize).next_multiple_of(16),
            (size[1] as usize).next_multiple_of(16),
        ];
        let coarse_size = [
            (fine_size[0] / 4).next_multiple_of(16),
            (fine_size[1] / 4).next_multiple_of(16),
        ];
        let level = |size: [usize; 2]| -> Result<Level, String> {
            let format = ImageFormat {
                order: CHANNEL_R,
                kind: UNORM_INT8,
            };
            let mut code = SUCCESS;
            let mut image = || {
                let desc = ImageDesc {
                    kind: MEM_OBJECT_IMAGE2D,
                    width: size[0],
                    height: size[1],
                    depth: 0,
                    array_size: 0,
                    row_pitch: 0,
                    slice_pitch: 0,
                    mip_levels: 0,
                    samples: 0,
                    buffer: std::ptr::null_mut(),
                };
                // SAFETY: 有効なコンテキストとフォーマットと記述子。
                let image = unsafe {
                    (api.create_image)(
                        context,
                        MEM_READ_ONLY,
                        &format,
                        &desc,
                        std::ptr::null_mut(),
                        &mut code,
                    )
                };
                check("clCreateImage", code).map(|()| image)
            };
            let images = [image()?, image()?];
            // SAFETY: 有効なコンテキスト。
            let staging = unsafe {
                (api.create_buffer)(
                    context,
                    MEM_READ_ONLY | MEM_ALLOC_HOST_PTR,
                    size[0] * size[1],
                    std::ptr::null_mut(),
                    &mut code,
                )
            };
            check("clCreateBuffer（イメージへの書き込み）", code)?;
            let blocks = [size[0] / 4, size[1] / 4];
            let mut buffer = |bytes: usize| {
                // SAFETY: 有効なコンテキスト。
                let buffer = unsafe {
                    (api.create_buffer)(
                        context,
                        MEM_WRITE_ONLY | MEM_ALLOC_HOST_PTR,
                        bytes,
                        std::ptr::null_mut(),
                        &mut code,
                    )
                };
                check("clCreateBuffer", code).map(|()| buffer)
            };
            let vectors = [
                buffer(blocks[0] * blocks[1] * 4)?,
                buffer(blocks[0] * blocks[1] * 4)?,
            ];
            let residuals = [
                buffer(blocks[0] * blocks[1] * 2)?,
                buffer(blocks[0] * blocks[1] * 2)?,
            ];
            let plain_vectors = [
                buffer(blocks[0] * blocks[1] * 4)?,
                buffer(blocks[0] * blocks[1] * 4)?,
            ];
            let plain_residuals = [
                buffer(blocks[0] * blocks[1] * 2)?,
                buffer(blocks[0] * blocks[1] * 2)?,
            ];
            Ok(Level {
                size,
                blocks,
                images,
                staging,
                vectors,
                residuals,
                plain_vectors,
                plain_residuals,
            })
        };
        let fine = level(fine_size)?;
        let coarse = level(coarse_size)?;
        #[cfg(all(feature = "vaapi", target_os = "linux"))]
        let va = match std::mem::replace(&mut sharing, Err(String::new())) {
            Ok(opened) => {
                match va::Share::new(
                    &api,
                    context,
                    platform,
                    opened,
                    device,
                    [fine.size, coarse.size],
                ) {
                    Ok(share) => Some(share),
                    Err(why) => {
                        sharing = Err(why);
                        None
                    }
                }
            }
            Err(why) => {
                sharing = Err(why);
                None
            }
        };
        let macroblocks = (fine_size[0] / 16) * (fine_size[1] / 16);
        let mut prediction = || {
            // SAFETY: 有効なコンテキスト。
            let buffer = unsafe {
                (api.create_buffer)(
                    context,
                    MEM_READ_ONLY,
                    macroblocks * 4,
                    std::ptr::null_mut(),
                    &mut code,
                )
            };
            check("clCreateBuffer（予測）", code).map(|()| buffer)
        };
        let predictions = [prediction()?, prediction()?];
        let texture = |label, size: [u32; 2], format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0].max(1),
                    height: size[1].max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let as_u32 = |s: [usize; 2]| [s[0] as u32, s[1] as u32];
        let luma = Luma::new(
            device,
            [as_u32(fine.size), as_u32(coarse.size)],
            [size, [size[0].div_ceil(4), size[1].div_ceil(4)]],
        );
        let grid = [size[0].div_ceil(4), size[1].div_ceil(4)];
        let upload = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let raw = [
            texture(
                "zw vme forward",
                grid,
                wgpu::TextureFormat::Rg16Sint,
                upload,
            ),
            texture(
                "zw vme backward",
                grid,
                wgpu::TextureFormat::Rg16Sint,
                upload,
            ),
        ];
        let costs = [
            texture("zw vme cost", grid, wgpu::TextureFormat::R8Uint, upload),
            texture("zw vme cost", grid, wgpu::TextureFormat::R8Uint, upload),
        ];
        #[cfg(all(feature = "vaapi", target_os = "linux"))]
        let manner = match (&va, &sharing) {
            (Some(_), _) => String::from("、VA-API サーフェスをコピーなしで共有"),
            (None, Err(why)) => format!("、メインメモリ経由（VA-API で共有できない: {why}）"),
            (None, Ok(_)) => String::new(),
        };
        #[cfg(not(all(feature = "vaapi", target_os = "linux")))]
        let manner = "";
        tracing::info!(
            "Intel VME（OpenCL）: {name} {}x{}、グリッド 4（要求 {wanted}）、1/4 縮小で探索して等倍で精密化{manner}",
            size[0],
            size[1]
        );
        Ok(Self {
            api,
            context,
            queue,
            program,
            accelerators,
            _descs: descs,
            // SAFETY: `clReleaseAcceleratorINTEL_fn` の署名（無ければ解放しない）。
            release_accelerator: (!release_accelerator.is_null())
                .then(|| unsafe { std::mem::transmute(release_accelerator) }),
            size,
            wanted,
            fine,
            coarse,
            predictions,
            luma,
            raw,
            costs,
            has_prev: false,
            newest: 0,
            outside: 0.0,
            #[cfg(all(feature = "vaapi", target_os = "linux"))]
            va,
        })
    }

    /// 今のフレームの輝度を読み戻し、前々のフレームの所へアップロードする（前のフレームは残すので一回に 1 フレーム）。
    fn read_luma(&mut self, gpu: &mut Gpu<'_>, source: &wgpu::Texture) -> Result<(), String> {
        #[cfg(all(feature = "vaapi", target_os = "linux"))]
        if let Some(share) = &self.va {
            self.luma.encode(gpu, source, false);
            self.newest ^= 1;
            share.record(gpu, &self.luma.written, self.newest);
            gpu.flush();
            gpu.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| format!("輝度の書き込みの待機：{e}"))?;
            let targets = [
                (self.fine.images[self.newest], self.fine.size),
                (self.coarse.images[self.newest], self.coarse.size),
            ];
            return share.bring(&self.api, self.queue, self.newest, targets);
        }
        self.luma.encode(gpu, source, true);
        gpu.flush();
        for buffer in &self.luma.readback {
            buffer.map_async(wgpu::MapMode::Read, .., |_| {});
        }
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        self.newest ^= 1;
        let slot = self.newest;
        for (k, level) in [&self.fine, &self.coarse].into_iter().enumerate() {
            let readback = &self.luma.readback[k];
            let written = {
                let view = readback
                    .get_mapped_range(..)
                    .map_err(|e| format!("輝度を読み戻せない：{e:?}"))?;
                if trace() {
                    let sum: u64 = view.iter().map(|&v| u64::from(v)).sum();
                    tracing::info!(
                        "Intel VME: レベル {k} の輝度合計 {sum}（{}x{}）",
                        level.size[0],
                        level.size[1]
                    );
                }
                self.map::<u8>(level.staging, view.len(), MAP_WRITE_INVALIDATE_REGION)
                    .map(|mut staging| staging.copy_from_slice(&view))
            };
            readback.unmap();
            written?;
            let origin = [0usize; 3];
            let region = [level.size[0], level.size[1], 1];
            // SAFETY: 有効なキューとバッファとイメージ、大きさは両方に合う。インオーダーのキュー。
            check("clEnqueueCopyBufferToImage", unsafe {
                (self.api.copy_to_image)(
                    self.queue,
                    level.staging,
                    level.images[slot],
                    0,
                    origin.as_ptr(),
                    region.as_ptr(),
                    0,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            })?;
        }
        Ok(())
    }

    /// カーネルは探索のたびに作る（最後に作ったカーネルしか正しく探索しない）。
    fn search(&self, l: usize, direction: usize, predicted: bool) -> Result<Handle, String> {
        let api = &self.api;
        let level = if l == 0 { &self.coarse } else { &self.fine };
        let mut code = SUCCESS;
        // SAFETY: 有効なプログラムと NUL 終端の名前。
        let kernel = unsafe { (api.create_kernel)(self.program, KERNEL.as_ptr(), &mut code) };
        check("clCreateKernel", code)?;
        let (cur, prev) = (self.newest, self.newest ^ 1);
        let (src, reference) = if direction == 0 {
            (prev, cur)
        } else {
            (cur, prev)
        };
        let set = |index: u32, value: Option<&Handle>| {
            let pointer = value.map_or(std::ptr::null(), |v| std::ptr::from_ref(v).cast());
            // SAFETY: 有効なカーネルと、引数の大きさの値（無しなら NULL のバッファ）。
            check("clSetKernelArg", unsafe {
                (api.set_arg)(kernel, index, size_of::<Handle>(), pointer)
            })
        };
        let result = (|| {
            set(0, Some(&self.accelerators[usize::from(predicted)]))?;
            set(1, Some(&level.images[src]))?;
            set(2, Some(&level.images[reference]))?;
            set(3, predicted.then_some(&self.predictions[direction]))?;
            if l == 1 && !predicted {
                set(4, Some(&level.plain_vectors[direction]))?;
                set(5, Some(&level.plain_residuals[direction]))?;
            } else {
                set(4, Some(&level.vectors[direction]))?;
                set(5, Some(&level.residuals[direction]))?;
            }
            let global = level.size;
            // SAFETY: 有効なキューと、引数を設定したカーネル。
            check("clEnqueueNDRangeKernel", unsafe {
                (api.run)(
                    self.queue,
                    kernel,
                    2,
                    std::ptr::null(),
                    global.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            })
        })();
        match result {
            Ok(()) => Ok(kernel),
            Err(e) => {
                self.settle(&[kernel]);
                Err(e)
            }
        }
    }

    fn settle(&self, kernels: &[Handle]) {
        // SAFETY: 有効なキューと、作ったカーネルを一度ずつ解放する。
        unsafe {
            (self.api.finish)(self.queue);
            for &kernel in kernels {
                (self.api.release_kernel)(kernel);
            }
        }
    }

    fn search_all(&self, searches: &[(usize, usize, bool)]) -> Result<(), String> {
        let mut kernels = Vec::with_capacity(searches.len());
        let mut failed = None;
        for &(l, direction, predicted) in searches {
            match self.search(l, direction, predicted) {
                Ok(kernel) => kernels.push(kernel),
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        self.settle(&kernels);
        failed.map_or(Ok(()), Err)
    }

    fn map<T: Copy>(
        &self,
        buffer: Handle,
        count: usize,
        flags: u64,
    ) -> Result<Mapped<'_, T>, String> {
        let mut code = SUCCESS;
        // SAFETY: 有効なキューとバッファ。大きさはバッファを作ったときの大きさ以下。
        let pointer = unsafe {
            (self.api.map_buffer)(
                self.queue,
                buffer,
                TRUE,
                flags,
                0,
                count * size_of::<T>(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut code,
            )
        };
        check("clEnqueueMapBuffer", code)?;
        Ok(Mapped {
            flow: self,
            buffer,
            pointer: pointer.cast(),
            count,
        })
    }

    fn predictions(&self, coarse: &[[i16; 2]]) -> Vec<[i16; 2]> {
        // 1/4 の 4×4 ブロックは等倍の 16×16 に当たる。予測は等倍の 1/4 画素で（×4）。
        let mbs = [self.fine.size[0] / 16, self.fine.size[1] / 16];
        let mut predictions = vec![[0i16; 2]; mbs[0] * mbs[1]];
        // 予測は周り 3×3 の中央値（1/4 レベルの孤立した外れ値を消す）。
        let [cw, ch] = self.coarse.blocks;
        let width = mbs[0];
        let mut lanes = [(); 9].map(|()| vec![0i16; width]);
        for y in 0..mbs[1] {
            let rows = [y.saturating_sub(1), y.min(ch - 1), (y + 1).min(ch - 1)]
                .map(|r| &coarse[r * cw..][..cw]);
            for component in 0..2 {
                for (i, lane) in lanes.iter_mut().enumerate() {
                    let row = rows[i / 3];
                    let dx = i % 3;
                    for (x, out) in lane.iter_mut().enumerate() {
                        let c = (x + dx).saturating_sub(1).min(cw - 1);
                        *out = row[c][component];
                    }
                }
                median9_lanes(&mut lanes);
                let (scale, low, high) = if component == 0 {
                    (4, -2048, 2047)
                } else {
                    (4, -512, 511)
                };
                for (x, &m) in lanes[4].iter().enumerate() {
                    predictions[y * width + x][component] =
                        m.saturating_mul(scale).clamp(low, high);
                }
            }
        }
        predictions
    }

    #[allow(clippy::type_complexity)]
    fn directions(&self) -> Result<[(Vec<[i16; 2]>, Vec<u16>); 2], String> {
        self.search_all(&[(0, 0, false), (0, 1, false)])?;
        let coarse_count = self.coarse.blocks[0] * self.coarse.blocks[1];
        let mut predictions = Vec::with_capacity(2);
        let mut heads = Vec::with_capacity(2);
        for direction in 0..2 {
            let coarse =
                self.map::<[i16; 2]>(self.coarse.vectors[direction], coarse_count, MAP_READ)?;
            predictions.push(self.predictions(&coarse));
            heads.push(coarse[..4.min(coarse_count)].to_vec());
        }
        for (direction, p) in predictions.iter().enumerate() {
            // SAFETY: 有効なキューとバッファ。`predictions` は等倍の探索を待つまで有効。
            check("clEnqueueWriteBuffer（予測）", unsafe {
                (self.api.write_buffer)(
                    self.queue,
                    self.predictions[direction],
                    FALSE,
                    0,
                    size_of_val(p.as_slice()),
                    p.as_ptr().cast(),
                    0,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                )
            })?;
        }
        // 等倍で予測の周りと 0 の周りを探索し、誤差の小さい方を採る（予測は外れることがある）。
        self.search_all(&[(1, 0, true), (1, 0, false), (1, 1, true), (1, 1, false)])?;
        let count = self.fine.blocks[0] * self.fine.blocks[1];
        let mut out: [(Vec<[i16; 2]>, Vec<u16>); 2] = Default::default();
        for (direction, (vectors, residuals)) in out.iter_mut().enumerate() {
            let level = &self.fine;
            let predicted = self.map::<[i16; 2]>(level.vectors[direction], count, MAP_READ)?;
            let predicted_residuals =
                self.map::<u16>(level.residuals[direction], count, MAP_READ)?;
            let plain = self.map::<[i16; 2]>(level.plain_vectors[direction], count, MAP_READ)?;
            let plain_residuals =
                self.map::<u16>(level.plain_residuals[direction], count, MAP_READ)?;
            let pairs = predicted.iter().zip(predicted_residuals.iter());
            let plains = plain.iter().zip(plain_residuals.iter());
            (*vectors, *residuals) = pairs
                .zip(plains)
                .map(|((&v, &r), (&pv, &pr))| if pr < r { (pv, pr) } else { (v, r) })
                .unzip();
            if trace() {
                let moving = vectors.iter().filter(|v| v[0] != 0 || v[1] != 0).count();
                tracing::info!(
                    "Intel VME: 方向 {direction}: 縮小レベルの先頭 {:?}、等倍の先頭 {:?}、動きのあるブロック {moving}/{count}、残差の最大 {}",
                    heads[direction],
                    &vectors[..4.min(count)],
                    residuals.iter().max().copied().unwrap_or(0)
                );
            }
        }
        Ok(out)
    }

    fn upload(&self, gpu: &Gpu<'_>, k: usize, vectors: &[[i16; 2]], residuals: &[u16]) {
        let grid = [self.raw[k].width() as usize, self.raw[k].height() as usize];
        let pitch = self.fine.blocks[0];
        let mut flow = vec![0u8; grid[0] * grid[1] * 4];
        let mut cost = vec![0u8; grid[0] * grid[1]];
        let rows = flow
            .chunks_exact_mut(grid[0] * 4)
            .zip(cost.chunks_exact_mut(grid[0]));
        for (y, (flow, cost)) in rows.enumerate() {
            let at = y * pitch;
            let vectors = &vectors[at..at + grid[0]];
            let residuals = &residuals[at..at + grid[0]];
            for (out, v) in flow.as_chunks_mut::<4>().0.iter_mut().zip(vectors) {
                let [x, y] = v.map(|c| c.saturating_mul(8).to_le_bytes());
                *out = [x[0], x[1], y[0], y[1]];
            }
            for (out, &r) in cost.iter_mut().zip(residuals) {
                *out = (r / 16).min(255) as u8;
            }
        }
        let write = |texture: &wgpu::Texture, data: &[u8], bytes: u32| {
            gpu.queue.write_texture(
                texture.as_image_copy(),
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(grid[0] as u32 * bytes),
                    rows_per_image: Some(grid[1] as u32),
                },
                texture.size(),
            );
        };
        write(&self.raw[k], &flow, 4);
        write(&self.costs[k], &cost, 1);
    }

    fn run_inner(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        if !run.new_input {
            return Ok(());
        }
        // 先に送ったコマンドが終わるのを待ってから計る（その時間は各パスの計時に入っている）。
        gpu.flush();
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let began = std::time::Instant::now();
        let mut marks: Vec<(&str, std::time::Instant)> = Vec::new();
        self.read_luma(gpu, run.cur)?;
        marks.push(("読み戻してアップロード", std::time::Instant::now()));
        if !self.has_prev {
            self.has_prev = true;
            convert.clear(gpu, run.target);
            return Ok(());
        }
        let [(forward, forward_cost), (backward, backward_cost)] = self.directions()?;
        marks.push(("探索", std::time::Instant::now()));
        self.upload(gpu, 0, &forward, &forward_cost);
        self.upload(gpu, 1, &backward, &backward_cost);
        convert.flow(gpu, &self.raw[0], &self.raw[1], run.target);
        marks.push(("wgpu へ", std::time::Instant::now()));
        if trace() {
            let mut last = began;
            let parts: Vec<String> = marks
                .iter()
                .map(|(name, at)| {
                    let ms = at.duration_since(last).as_secs_f64() * 1000.0;
                    last = *at;
                    format!("{name} {ms:.2}")
                })
                .collect();
            tracing::info!("Intel VME: 内訳（ms）: {}", parts.join("、"));
        }
        let ms = began.elapsed().as_secs_f64() * 1000.0;
        self.outside = if self.outside > 0.0 {
            self.outside * 0.8 + ms * 0.2
        } else {
            ms
        };
        Ok(())
    }
}

impl super::flow::Provider for Flow {
    fn size(&self) -> [u32; 2] {
        self.size
    }

    fn wanted_grid(&self) -> u32 {
        self.wanted
    }

    fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        self.run_inner(gpu, convert, run)
    }

    fn confidence(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        target: &wgpu::Texture,
    ) -> bool {
        convert.cost(gpu, &self.costs[0], &self.costs[1], target);
        true
    }

    fn outside_ms(&self) -> f64 {
        self.outside
    }
}

// SAFETY: 生のポインターはこの `Flow` だけが持つ OpenCL のオブジェクトで、使うのは `&mut self` の一つのスレッドだけ。
unsafe impl Send for Flow {}

impl Drop for Flow {
    fn drop(&mut self) {
        let api = self.api;
        // SAFETY: 作ったオブジェクトを一度ずつ、使う側から先に解放する。
        unsafe {
            (api.finish)(self.queue);
            for level in [&self.fine, &self.coarse] {
                for &image in &level.images {
                    (api.release_mem)(image);
                }
                (api.release_mem)(level.staging);
                for k in 0..2 {
                    (api.release_mem)(level.vectors[k]);
                    (api.release_mem)(level.residuals[k]);
                    (api.release_mem)(level.plain_vectors[k]);
                    (api.release_mem)(level.plain_residuals[k]);
                }
            }
            for k in 0..2 {
                (api.release_mem)(self.predictions[k]);
            }

            (api.release_program)(self.program);
            if let Some(release) = self.release_accelerator {
                for &accelerator in &self.accelerators {
                    release(accelerator);
                }
            }
            (api.release_queue)(self.queue);
            (api.release_context)(self.context);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::median9_lanes;

    #[test]
    fn the_median_network_agrees_with_sorting() {
        let mut seed = 0x2545_f491_u32;
        for _ in 0..10_000 {
            let mut p = [0i16; 9];
            for v in &mut p {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *v = (seed % 64) as i16 - 32;
            }
            let mut sorted = p;
            sorted.sort_unstable();
            let mut lanes = p.map(|v| vec![v]);
            median9_lanes(&mut lanes);
            assert_eq!(lanes[4][0], sorted[4], "{p:?}");
        }
    }
}
