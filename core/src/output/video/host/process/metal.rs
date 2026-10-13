// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `metal` の転送方式（macOS）：IOSurface（入力のスロットと出力のスロット）を Metal のテクスチャとしてもラップする。
//! IOSurface は mach ポートで渡す（子が check_in したポートを本体が look_up する）。
//! 同期はメッセージの通知で行う（MTLSharedEvent はプロセスを越えるのに XPC が要るので使わない）。

use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicU32, Ordering};

use objc2::encode::{Encode, Encoding};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ProtocolObject};
use objc2_metal::{MTLDevice, MTLTexture, MTLTextureType};
use wgpu::hal::api::Metal as Api;
use zwsdk::video::process::{self as wire, Format};

use super::super::Gpu;
use super::{Handed, Link};
use crate::decode::videotoolbox::vt;

const PIXEL_F32: u32 = 0x4C30_3066; // 'L00f'
const PIXEL_F16: u32 = 0x4C30_3068; // 'L00h'
const MTL_R32_FLOAT: usize = 55;
const MTL_R16_FLOAT: usize = 25;
const ROW_ALIGN: u32 = 256;

const MACH_SEND_MSG: i32 = 0x0000_0001;
const MACH_SEND_TIMEOUT: i32 = 0x0000_0010;
const MACH_MSG_TYPE_MOVE_SEND: u8 = 17;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;
const MACH_MSG_PORT_DESCRIPTOR: u8 = 0;

#[cfg_attr(target_vendor = "apple", link(name = "IOSurface", kind = "framework"))]
unsafe extern "C" {
    fn IOSurfaceCreate(properties: vt::CFDictionaryRef) -> *mut c_void;
    fn IOSurfaceCreateMachPort(surface: *mut c_void) -> u32;
    fn IOSurfaceGetBytesPerRow(surface: *mut c_void) -> usize;
    fn IOSurfaceGetAllocSize(surface: *mut c_void) -> usize;
    static kIOSurfaceWidth: vt::CFStringRef;
    static kIOSurfaceHeight: vt::CFStringRef;
    static kIOSurfaceBytesPerElement: vt::CFStringRef;
    static kIOSurfaceBytesPerRow: vt::CFStringRef;
    static kIOSurfacePixelFormat: vt::CFStringRef;
}

unsafe extern "C" {
    static bootstrap_port: u32;
    static mach_task_self_: u32;
    fn bootstrap_look_up(bp: u32, name: *const c_char, sp: *mut u32) -> i32;
    fn mach_msg(
        msg: *mut c_void,
        option: i32,
        send_size: u32,
        rcv_size: u32,
        rcv_name: u32,
        timeout: u32,
        notify: u32,
    ) -> i32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
}

/// ポートの記述子を一つ持つメッセージ（4 バイト境界でパックした並び）。
#[repr(C, packed(4))]
struct PortMessage {
    bits: u32,
    size: u32,
    remote: u32,
    local: u32,
    voucher: u32,
    id: i32,
    descriptors: u32,
    name: u32,
    pad1: u32,
    pad2: u16,
    disposition: u8,
    kind: u8,
}

/// `IOSurfaceRef`（Objective-C の型の記述は `^{__IOSurface=}`）。
#[repr(transparent)]
#[derive(Clone, Copy)]
struct SurfaceRef(*mut c_void);

// SAFETY: 中身は `struct __IOSurface *` そのもの。
unsafe impl Encode for SurfaceRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("__IOSurface", &[]));
}

pub(super) fn id(device: &wgpu::Device) -> Option<Vec<u8>> {
    let raw = raw_device(device)?;
    Some(raw.registryID().to_le_bytes().to_vec())
}

fn raw_device(device: &wgpu::Device) -> Option<Retained<ProtocolObject<dyn MTLDevice>>> {
    // SAFETY: as_hal はデバイスを貸すだけ。ポインターは参照カウント付きのコピー。
    unsafe { device.as_hal::<Api>().map(|hal| hal.raw_device().clone()) }
}

pub(super) fn service_name() -> String {
    static SERIAL: AtomicU32 = AtomicU32::new(0);
    format!(
        "jp.zerowave.process.{}.{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) fn look_up(name: &str) -> Result<u32, String> {
    let name = CString::new(name).map_err(|e| e.to_string())?;
    let mut port = 0u32;
    // SAFETY: 名前は C の文字列、出力はローカル変数。
    let status = unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &raw mut port) };
    if status != 0 {
        return Err(format!(
            "bootstrap_look_up（{}）：{status}（子が OPEN の応答の前に bootstrap_check_in していない）",
            name.to_string_lossy()
        ));
    }
    Ok(port)
}

pub(super) fn release_port(port: u32) {
    // SAFETY: こちらが持つ送信権を一つ解放する。
    unsafe {
        mach_port_deallocate(mach_task_self_, port);
    }
}

fn send_port(remote: u32, id: u32, port: u32) -> Result<(), String> {
    let mut message = PortMessage {
        bits: MACH_MSG_TYPE_COPY_SEND | MACH_MSGH_BITS_COMPLEX,
        size: size_of::<PortMessage>() as u32,
        remote,
        local: 0,
        voucher: 0,
        id: id as i32,
        descriptors: 1,
        name: port,
        pad1: 0,
        pad2: 0,
        disposition: MACH_MSG_TYPE_MOVE_SEND,
        kind: MACH_MSG_PORT_DESCRIPTOR,
    };
    // SAFETY: ローカルのメッセージ。送れば port の権利は相手へ移る。
    let status = unsafe {
        mach_msg(
            (&raw mut message).cast(),
            MACH_SEND_MSG | MACH_SEND_TIMEOUT,
            size_of::<PortMessage>() as u32,
            0,
            0,
            2000,
            0,
        )
    };
    if status != 0 {
        release_port(port);
        return Err(format!(
            "IOSurface のポートを送れない（mach_msg {status:#x}）"
        ));
    }
    Ok(())
}

struct Surface {
    surface: vt::Owned<c_void>,
    texture: wgpu::Texture,
}

pub(super) struct Metal {
    inputs: Vec<Surface>,
    middles: Vec<Surface>,
    /// 入力を書き込む先と、中間フレームを読み出す先（行の幅は [`ROW_ALIGN`] の倍数）。
    packed: Vec<wgpu::Buffer>,
    taken: Vec<wgpu::Buffer>,
    stride: u32,
    size: [u32; 2],
    value: u64,
}

impl Metal {
    pub fn new(
        device: &wgpu::Device,
        format: Format,
        size: [u32; 2],
        inputs: usize,
        middles: usize,
    ) -> Result<Self, String> {
        let raw = raw_device(device).ok_or("Metal のデバイスではない")?;
        let stride = (size[0] * format.bytes()).next_multiple_of(ROW_ALIGN);
        let bytes = wire::picture_bytes(size[1], stride);
        let surfaces = |count: usize, label: &str| -> Result<Vec<Surface>, String> {
            (0..count)
                .map(|_| surface(device, &raw, format, size, stride, label))
                .collect()
        };
        let buffers = |count: usize, label, usage| -> Vec<wgpu::Buffer> {
            (0..count)
                .map(|_| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size: bytes,
                        usage: wgpu::BufferUsages::STORAGE | usage,
                        mapped_at_creation: false,
                    })
                })
                .collect()
        };
        Ok(Self {
            inputs: surfaces(inputs, "zw process input")?,
            middles: surfaces(middles, "zw process mid")?,
            packed: buffers(inputs, "zw process input", wgpu::BufferUsages::COPY_SRC),
            taken: buffers(middles, "zw process mid", wgpu::BufferUsages::COPY_DST),
            stride,
            size,
            value: 0,
        })
    }

    fn layout(&self) -> wgpu::TexelCopyBufferLayout {
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(self.stride),
            rows_per_image: Some(3 * self.size[1]),
        }
    }

    fn extent(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.size[0],
            height: 3 * self.size[1],
            depth_or_array_layers: 1,
        }
    }
}

fn surface(
    device: &wgpu::Device,
    raw: &ProtocolObject<dyn MTLDevice>,
    format: Format,
    size: [u32; 2],
    stride: u32,
    label: &str,
) -> Result<Surface, String> {
    let (pixel, metal_format, wgpu_format) = match format {
        Format::F32 => (PIXEL_F32, MTL_R32_FLOAT, wgpu::TextureFormat::R32Float),
        Format::F16 => (PIXEL_F16, MTL_R16_FLOAT, wgpu::TextureFormat::R16Float),
    };
    let height = 3 * size[1];
    let numbers = [
        size[0] as i32,
        height as i32,
        format.bytes() as i32,
        stride as i32,
        pixel as i32,
    ]
    .map(vt::number);
    let [Some(w), Some(h), Some(bpe), Some(bpr), Some(pf)] = numbers else {
        return Err(String::from("CFNumber が作れない"));
    };
    // SAFETY: キーは IOSurface の外部シンボル、値はいま作った物。
    let surface = unsafe {
        let properties = vt::dictionary(&[
            (kIOSurfaceWidth, w.as_ptr().cast_const()),
            (kIOSurfaceHeight, h.as_ptr().cast_const()),
            (kIOSurfaceBytesPerElement, bpe.as_ptr().cast_const()),
            (kIOSurfaceBytesPerRow, bpr.as_ptr().cast_const()),
            (kIOSurfacePixelFormat, pf.as_ptr().cast_const()),
        ])
        .ok_or("CFDictionary が作れない")?;
        vt::Owned::new(IOSurfaceCreate(properties.as_ptr().cast_const()))
            .ok_or("IOSurface が作れない")?
    };
    // SAFETY: 確かめたクラスの、ヘッダーどおりの呼び出し。
    let texture = unsafe {
        let class = AnyClass::get(c"MTLTextureDescriptor").ok_or("MTLTextureDescriptor が無い")?;
        let descriptor: Option<Retained<AnyObject>> = msg_send![
            class,
            texture2DDescriptorWithPixelFormat: metal_format,
            width: size[0] as usize,
            height: height as usize,
            mipmapped: Bool::NO
        ];
        let descriptor = descriptor.ok_or("MTLTextureDescriptor が作れない")?;
        let _: () = msg_send![&*descriptor, setUsage: 3usize];
        let texture: Option<Retained<ProtocolObject<dyn MTLTexture>>> = msg_send![
            raw,
            newTextureWithDescriptor: &*descriptor,
            iosurface: SurfaceRef(surface.as_ptr()),
            plane: 0usize
        ];
        texture.ok_or("IOSurface の Metal のテクスチャが作れない")?
    };
    let extent = wgpu::Extent3d {
        width: size[0],
        height,
        depth_or_array_layers: 1,
    };
    // SAFETY: いまこのデバイスのために作った 2D の 1 プレーンのサーフェス。フォーマットと大きさは作ったとおり。
    let texture = unsafe {
        let hal = wgpu::hal::metal::Device::texture_from_raw(
            texture,
            wgpu_format,
            MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width: size[0],
                height,
                depth: 1,
            },
            None,
        );
        device.create_texture_from_hal::<Api>(
            hal,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::TextureUses::UNINITIALIZED,
        )
    };
    Ok(Surface { surface, texture })
}

impl super::Way for Metal {
    fn stride(&self) -> u32 {
        self.stride
    }

    fn input(&self, slot: usize) -> &wgpu::Buffer {
        &self.packed[slot]
    }

    fn output(&self, slot: usize) -> &wgpu::Buffer {
        &self.taken[slot]
    }

    fn share(&mut self, link: &mut Link) -> Result<(), String> {
        let remote = link.port.ok_or("子のポートが無い")?;
        for (what, surfaces) in [(wire::INPUT, &self.inputs), (wire::MIDDLE, &self.middles)] {
            for (index, surface) in (0..).zip(surfaces) {
                let raw = surface.surface.as_ptr();
                // SAFETY: 生きている IOSurface を読むだけ。ポートは送信権を一つ作る。
                let (stride, size, port) = unsafe {
                    (
                        IOSurfaceGetBytesPerRow(raw) as u32,
                        IOSurfaceGetAllocSize(raw) as u64,
                        IOSurfaceCreateMachPort(raw),
                    )
                };
                send_port(remote, wire::port_id(what, index), port)?;
                link.share(wire::Share {
                    what,
                    index,
                    stride,
                    size,
                    handle: 0,
                })?;
            }
        }
        Ok(())
    }

    fn hand_over(&mut self, gpu: &mut Gpu<'_>, packed: &[usize]) -> Result<Handed, String> {
        for &slot in packed {
            gpu.encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.packed[slot],
                    layout: self.layout(),
                },
                self.inputs[slot].texture.as_image_copy(),
                self.extent(),
            );
        }
        let after = gpu.flush();
        self.value += 1;
        Ok(Handed {
            value: self.value,
            after: Some((after, None)),
        })
    }

    fn done(&self, _value: u64) -> Option<bool> {
        None
    }

    fn take_back(&mut self, gpu: &mut Gpu<'_>, _value: u64, slot: usize) -> Result<(), String> {
        gpu.encoder.copy_texture_to_buffer(
            self.middles[slot].texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.taken[slot],
                layout: self.layout(),
            },
            self.extent(),
        );
        Ok(())
    }
}
