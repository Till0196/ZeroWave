// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! VideoToolbox のフレーム補間（VTFrameProcessor の FrameRateConversion）。
//! IOSurface 付きの `CVPixelBuffer` を wgpu のテクスチャとしてもラップする。別のテクスチャなので Metal の
//! 依存関係の追跡が効かず、共有イベントで順序を守る（`enable_strict_event_sync`）。

use std::ffi::{CStr, c_void};

use objc2::encode::{Encode, Encoding};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ProtocolObject};
use objc2::{msg_send, sel};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLDevice, MTLSharedEvent, MTLTexture, MTLTextureType,
};
use wgpu::hal::api::Metal;

use super::{Convert, Gpu, Run};
use crate::decode::videotoolbox::vt;

/// 最初の数回と、以後はこの回数おきに時間を測る。
const MEASURE_FIRST: u64 = 8;
const MEASURE_EVERY: u64 = 64;

/// 値渡しなので型エンコーディングが要る。
#[repr(C)]
#[derive(Clone, Copy)]
struct CmTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: `CMTime` のメモリレイアウトそのもの（`{?=qiIq}`）。
unsafe impl Encode for CmTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

impl CmTime {
    fn new(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: 1,
            epoch: 0,
        }
    }
}

#[repr(transparent)]
#[derive(Clone, Copy)]
struct CvBuffer(*mut c_void);

// SAFETY: 中身は `struct __CVBuffer *` そのもの。
unsafe impl Encode for CvBuffer {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("__CVBuffer", &[]));
}

const QUALITY_NORMAL: isize = 1;
const SEQUENTIAL: isize = 2;
const SAME_REFERENCES: isize = 3;

fn class(name: &CStr) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

fn answers(class: &AnyClass, selector: objc2::runtime::Sel) -> bool {
    // SAFETY: NSObject の respondsToSelector:。
    let yes: Bool = unsafe { msg_send![class, respondsToSelector: selector] };
    yes.as_bool()
}

pub fn supported() -> bool {
    let (Some(_), Some(config)) = (
        class(c"VTFrameProcessor"),
        class(c"VTFrameRateConversionConfiguration"),
    ) else {
        return false;
    };
    let processor = class(c"VTFrameProcessor").expect("checked");
    // SAFETY: インスタンスメソッドがあるかを問い合わせるだけ。
    let has_command_buffer: Bool = unsafe {
        msg_send![processor, instancesRespondToSelector: sel!(processWithCommandBuffer:parameters:)]
    };
    if !has_command_buffer.as_bool() {
        return false;
    }
    // isSupported と processorSupported は応答する方を使う。
    if answers(config, sel!(isSupported)) {
        // SAFETY: 応答することを確かめた。
        let yes: Bool = unsafe { msg_send![config, isSupported] };
        yes.as_bool()
    } else if answers(config, sel!(processorSupported)) {
        // SAFETY: 応答することを確かめた。こちらは Boolean（unsigned char）。
        let yes: u8 = unsafe { msg_send![config, processorSupported] };
        yes != 0
    } else {
        false
    }
}

struct Slot {
    buffer: vt::Owned<c_void>,
    /// `texture` より長く持つ。
    _metal: vt::Owned<c_void>,
    texture: wgpu::Texture,
}

/// 端からこれより近い位相は、補間せずに前か今のフレームをコピーする。
const NEAR: f32 = 0.02;

/// モデルは解像度で速度が大きく違うので、速い解像度へスケーリングして渡し、元の解像度へ戻す。
fn work_size(size: [u32; 2]) -> [u32; 2] {
    if size[1] > 540 {
        [1920, 1080]
    } else {
        [960, 540]
    }
}

pub struct Frc {
    processor: Retained<AnyObject>,
    _config: Retained<AnyObject>,
    _cache: vt::Owned<c_void>,
    size: [u32; 2],
    /// 前と今を読んでいる間に、次のフレームは 3 つ目のバッファへ書く。
    slots: [Slot; 3],
    outs: [Slot; 2],
    cur: usize,
    made: usize,
    /// バッファそのものにはレンダリングしない。
    staging: wgpu::Texture,
    has_prev: bool,
    /// 0 なら Sequential、以後は同じ参照。
    runs: u32,
    clock: i64,
    event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    value: u64,
    /// wgpu のキューに混ぜると表示のサブミットが後ろに並ぶ。
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    /// wgpu の外なのでパスごとの計時に入らない。
    calls: u64,
    outside: f64,
}

// SAFETY: プロセッサー・設定・キューは所有者が `&mut` で一度に一つのスレッドから使うだけで、
// ObjC の参照カウントはスレッドセーフ。
unsafe impl Send for Frc {}

impl Frc {
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    pub fn outside_ms(&self) -> f64 {
        self.outside
    }

    /// VideoToolbox は自分のコマンドバッファで処理するので、実時間を測る。待つ間はレンダリングスレッドが止まる。
    fn measure(&mut self, done: u64, sent: std::time::Instant) {
        let limit = std::time::Duration::from_millis(200);
        loop {
            // SAFETY: 有効な共有イベント。`signaledValue` は uint64_t を返す。
            let reached: u64 = unsafe { msg_send![&*self.event, signaledValue] };
            if reached >= done {
                break;
            }
            if sent.elapsed() > limit {
                return;
            }
            std::thread::sleep(std::time::Duration::from_micros(250));
        }
        let ms = sent.elapsed().as_secs_f64() * 1000.0;
        self.outside = if self.outside > 0.0 {
            self.outside * 0.8 + ms * 0.2
        } else {
            ms
        };
    }

    pub fn open(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
    ) -> Result<Self, String> {
        let work = work_size(size);
        if !supported() {
            return Err(String::from(
                "この Mac では VideoToolbox のフレーム補間が使えない（macOS 15.4 以降の Apple シリコン）",
            ));
        }
        let (width, height) = (work[0] as isize, work[1] as isize);
        let config_class = class(c"VTFrameRateConversionConfiguration")
            .ok_or("VTFrameRateConversionConfiguration が無い")?;
        let processor_class = class(c"VTFrameProcessor").ok_or("VTFrameProcessor が無い")?;
        // SAFETY: 存在を確かめたクラスの、ヘッダーどおりの呼び出し。
        let (config, processor) = unsafe {
            let revision: isize = msg_send![config_class, defaultRevision];
            let alloc: Allocated<AnyObject> = msg_send![config_class, alloc];
            let config: Option<Retained<AnyObject>> = msg_send![
                alloc,
                initWithFrameWidth: width,
                frameHeight: height,
                usePrecomputedFlow: Bool::NO,
                qualityPrioritization: QUALITY_NORMAL,
                revision: revision
            ];
            let config = config.ok_or_else(|| {
                format!("VideoToolbox の設定を作れない（{}x{}）", work[0], work[1])
            })?;
            let alloc: Allocated<AnyObject> = msg_send![processor_class, alloc];
            let processor: Retained<AnyObject> = msg_send![alloc, init];
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let started: Bool =
                msg_send![&*processor, startSessionWithConfiguration: &*config, error: &mut error];
            if !started.as_bool() {
                return Err(String::from("VideoToolbox のフレーム補間を始められない"));
            }
            (config, processor)
        };
        // SAFETY: as_hal はデバイスを参照させるだけ。ポインターは参照カウント付きのコピー。
        let raw_device = unsafe { device.as_hal::<Metal>().map(|hal| hal.raw_device().clone()) }
            .ok_or("Metal のデバイスではない")?;
        let mut cache: vt::CVMetalTextureCacheRef = std::ptr::null_mut();
        // SAFETY: 有効な MTLDevice、出力先はローカル変数。
        let made = unsafe {
            vt::CVMetalTextureCacheCreate(
                vt::kCFAllocatorDefault,
                std::ptr::null(),
                Retained::as_ptr(&raw_device).cast_mut().cast(),
                std::ptr::null(),
                &raw mut cache,
            )
        };
        if made != 0 {
            return Err(format!("CVMetalTextureCacheCreate：CVReturn {made}"));
        }
        // SAFETY: Create が返したので所有権を持つ。
        let cache = unsafe { vt::Owned::new(cache) }.ok_or("CVMetalTextureCache が空")?;
        // バッファはプロセッサーが返す属性どおりに作り（「must conform to」）、Metal 互換のフラグだけ足す。
        let attributes = |selector| -> Result<Retained<AnyObject>, String> {
            // SAFETY: 設定の属性（NSDictionary）をコピーして、CoreVideo のキーを一つ足す。
            unsafe {
                let given: Option<Retained<AnyObject>> = match selector {
                    true => msg_send![&*config, sourcePixelBufferAttributes],
                    false => msg_send![&*config, destinationPixelBufferAttributes],
                };
                let given = given.ok_or("VideoToolbox がバッファの属性を返さない")?;
                let copy: Option<Retained<AnyObject>> = msg_send![&*given, mutableCopy];
                let copy = copy.ok_or("属性をコピーできない")?;
                let _: () = msg_send![&*copy, setObject: vt::kCFBooleanTrue.cast::<AnyObject>(), forKey: vt::kCVPixelBufferMetalCompatibilityKey.cast::<AnyObject>()];
                Ok(copy)
            }
        };
        let (source, destination) = (attributes(true)?, attributes(false)?);
        let slot =
            |attributes: &AnyObject, label| Self::slot(device, &cache, attributes, work, label);
        let staging = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zw vt staging"),
            size: wgpu::Extent3d {
                width: work[0],
                height: work[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let slots = [
            slot(&source, "zw vt a")?,
            slot(&source, "zw vt b")?,
            slot(&source, "zw vt c")?,
        ];
        let outs = [
            slot(&destination, "zw vt out a")?,
            slot(&destination, "zw vt out b")?,
        ];
        let event = raw_device
            .newSharedEvent()
            .ok_or("MTLSharedEvent が作れない")?;
        let own_queue = raw_device
            .newCommandQueue()
            .ok_or("Metal のキューが作れない")?;
        // SAFETY: キューを参照するだけ。待機を後続のコマンドバッファ全部に効かせる。
        unsafe {
            if let Some(hal) = queue.as_hal::<Metal>() {
                hal.enable_strict_event_sync()
                    .map_err(|e| format!("enable_strict_event_sync：{e}"))?;
            }
        }
        Ok(Self {
            processor,
            _config: config,
            _cache: cache,
            size,
            slots,
            outs,
            staging,
            cur: 0,
            made: 0,
            has_prev: false,
            runs: 0,
            clock: 0,
            event,
            value: 0,
            queue: own_queue,
            calls: 0,
            outside: 0.0,
        })
    }

    fn slot(
        device: &wgpu::Device,
        cache: &vt::Owned<c_void>,
        attributes: &AnyObject,
        size: [u32; 2],
        label: &'static str,
    ) -> Result<Slot, String> {
        let mut buffer: vt::CVPixelBufferRef = std::ptr::null_mut();
        // SAFETY: 有効な CFDictionary、出力先はローカル変数。
        let status = unsafe {
            vt::CVPixelBufferCreate(
                vt::kCFAllocatorDefault,
                size[0] as usize,
                size[1] as usize,
                vt::kCVPixelFormatType_64RGBAHalf,
                std::ptr::from_ref(attributes).cast(),
                &raw mut buffer,
            )
        };
        if status != 0 {
            return Err(format!("CVPixelBufferCreate（RGhA）：CVReturn {status}"));
        }
        // SAFETY: Create が返したので所有権を持つ。
        let buffer = unsafe { vt::Owned::new(buffer) }.ok_or("CVPixelBuffer が空")?;
        // 読むだけ（MTLTextureUsageShaderRead）。レンダーターゲットにしない（圧縮させない）。
        let usage = vt::number(1).ok_or("CFNumber が作れない")?;
        // SAFETY: キーは CoreVideo の外部シンボル、値はいま作ったオブジェクト。
        let texture_attributes =
            unsafe { vt::dictionary(&[(vt::kCVMetalTextureUsage, usage.as_ptr().cast_const())]) }
                .ok_or("CFDictionary を作れない")?;
        let mut metal: vt::CVMetalTextureRef = std::ptr::null_mut();
        // SAFETY: 有効なキャッシュとピクセルバッファ（単一プレーン）、出力先はローカル変数。
        let status = unsafe {
            vt::CVMetalTextureCacheCreateTextureFromImage(
                vt::kCFAllocatorDefault,
                cache.as_ptr(),
                buffer.as_ptr(),
                texture_attributes.as_ptr().cast_const(),
                vt::MTLPixelFormatRGBA16Float,
                size[0] as usize,
                size[1] as usize,
                0,
                &raw mut metal,
            )
        };
        if status != 0 {
            return Err(format!(
                "CVMetalTextureCacheCreateTextureFromImage（RGBA16Float）：CVReturn {status}"
            ));
        }
        // SAFETY: Create が返したので所有権を持つ。Get は借用なので、参照カウントを一つ上げて所有に変える。
        let (metal, texture) = unsafe {
            let metal = vt::Owned::new(metal).ok_or("CVMetalTexture が空")?;
            let raw = vt::CVMetalTextureGetTexture(metal.as_ptr());
            let texture = Retained::retain(raw.cast::<ProtocolObject<dyn MTLTexture>>())
                .ok_or("MTLTexture が空")?;
            (metal, texture)
        };
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        // SAFETY: いまこのデバイス用に作った単一プレーンの 2D テクスチャ。フォーマットと大きさは作ったとおり。
        let texture = unsafe {
            let hal = wgpu::hal::metal::Device::texture_from_raw(
                texture,
                wgpu::TextureFormat::Rgba16Float,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width: size[0],
                    height: size[1],
                    depth: 1,
                },
                None,
            );
            device.create_texture_from_hal::<Metal>(
                hal,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            )
        };
        Ok(Slot {
            buffer,
            _metal: metal,
            texture,
        })
    }

    pub fn run(
        &mut self,
        gpu: &mut Gpu<'_>,
        convert: &mut Convert,
        run: &Run<'_>,
    ) -> Result<(), String> {
        if run.new_input {
            self.has_prev = self.clock > 0;
            self.cur = (self.cur + 1) % self.slots.len();
            self.clock += 2;
            self.runs = 0;
            let slot = &self.slots[self.cur].texture;
            if run.cur.format() == slot.format() && run.cur.size() == slot.size() {
                convert.put(gpu, run.cur, slot);
            } else {
                convert.put(gpu, run.cur, &self.staging);
                gpu.encoder.copy_texture_to_texture(
                    self.staging.as_image_copy(),
                    slot.as_image_copy(),
                    slot.size(),
                );
            }
        }
        let prev = (self.cur + self.slots.len() - 1) % self.slots.len();
        // 端の位相はフィルターチェーンのフレームからコピーする（VideoToolbox のバッファに wgpu の処理を掛けない）。
        if !self.has_prev || run.phase >= 1.0 - NEAR {
            convert.put(gpu, run.cur, run.target);
            return Ok(());
        }
        if run.phase <= NEAR {
            convert.put(gpu, run.prev, run.target);
            return Ok(());
        }
        self.value += 1;
        let ready = self.value;
        // SAFETY: キューを参照するだけ。
        unsafe {
            gpu.queue
                .as_hal::<Metal>()
                .ok_or("Metal のキューではない")?
                .add_signal_event(self.event.clone(), ready);
        }
        gpu.flush();
        self.value += 1;
        let done = self.value;
        self.made ^= 1;
        let mode = if self.runs == 0 {
            SEQUENTIAL
        } else {
            SAME_REFERENCES
        };
        let command = self
            .queue
            .commandBuffer()
            .ok_or("Metal のコマンドバッファが作れない")?;
        command.encodeWaitForEvent_value(ProtocolObject::from_ref(&*self.event), ready);
        self.encode(&command, prev, run.phase, mode)?;
        command.encodeSignalEvent_value(ProtocolObject::from_ref(&*self.event), done);
        let sent = std::time::Instant::now();
        command.commit();
        self.calls += 1;
        if self.calls <= MEASURE_FIRST || self.calls.is_multiple_of(MEASURE_EVERY) {
            self.measure(done, sent);
        }
        self.runs += 1;
        // SAFETY: キューを参照するだけ。
        unsafe {
            gpu.queue
                .as_hal::<Metal>()
                .ok_or("Metal のキューではない")?
                .add_wait_event(self.event.clone(), done);
        }
        convert.put(gpu, &self.outs[self.made].texture, run.target);
        Ok(())
    }

    fn encode(
        &self,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
        prev: usize,
        phase: f32,
        mode: isize,
    ) -> Result<(), String> {
        let parameters = self.parameters(prev, phase, mode)?;
        // SAFETY: 有効なプロセッサー・コマンドバッファ・パラメーター。
        unsafe {
            let _: () = msg_send![&*self.processor, processWithCommandBuffer: command, parameters: &*parameters];
        }
        Ok(())
    }

    fn parameters(
        &self,
        prev: usize,
        phase: f32,
        mode: isize,
    ) -> Result<Retained<AnyObject>, String> {
        let frame_class = class(c"VTFrameProcessorFrame").ok_or("VTFrameProcessorFrame が無い")?;
        let parameters_class = class(c"VTFrameRateConversionParameters")
            .ok_or("VTFrameRateConversionParameters が無い")?;
        let number_class = class(c"NSNumber").ok_or("NSNumber が無い")?;
        let array_class = class(c"NSArray").ok_or("NSArray が無い")?;
        let frame = |slot: &Slot, at: i64| -> Result<Retained<AnyObject>, String> {
            // SAFETY: IOSurface 付きの有効なピクセルバッファ。
            unsafe {
                let alloc: Allocated<AnyObject> = msg_send![frame_class, alloc];
                let made: Option<Retained<AnyObject>> = msg_send![alloc, initWithBuffer: CvBuffer(slot.buffer.as_ptr()), presentationTimeStamp: CmTime::new(at, 60)];
                made.ok_or_else(|| String::from("VideoToolbox のフレームを作れない"))
            }
        };
        let source = frame(&self.slots[prev], self.clock - 2)?;
        let next = frame(&self.slots[self.cur], self.clock)?;
        let into = frame(&self.outs[self.made], self.clock - 1)?;
        // SAFETY: NSNumber と NSArray の生成（autorelease のオブジェクトを受け取る）。
        unsafe {
            let number: Retained<AnyObject> = msg_send![number_class, numberWithFloat: phase];
            let phases_ptr = [Retained::as_ptr(&number)];
            let phases: Retained<AnyObject> =
                msg_send![array_class, arrayWithObjects: phases_ptr.as_ptr(), count: 1usize];
            let into_ptr = [Retained::as_ptr(&into)];
            let destinations: Retained<AnyObject> =
                msg_send![array_class, arrayWithObjects: into_ptr.as_ptr(), count: 1usize];
            let alloc: Allocated<AnyObject> = msg_send![parameters_class, alloc];
            let parameters: Option<Retained<AnyObject>> = msg_send![
                alloc,
                initWithSourceFrame: &*source,
                nextFrame: &*next,
                opticalFlow: std::ptr::null::<AnyObject>(),
                interpolationPhase: &*phases,
                submissionMode: mode,
                destinationFrames: &*destinations
            ];
            parameters.ok_or_else(|| String::from("VideoToolbox のパラメーターを作れない"))
        }
    }
}

impl Drop for Frc {
    fn drop(&mut self) {
        // SAFETY: 始めた処理を閉じる。
        unsafe {
            let _: () = msg_send![&*self.processor, endSession];
        }
    }
}
