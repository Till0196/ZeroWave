// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! VA-API decoding. VA-API is a back half: the headers are read here with the same front half as
//! the software decoders, and surfaces come back as DMA-BUF (luma `R8`, chroma `GR88`).
//! レンダリングと違う GPU でデコードしたフレームは DMA-BUF で渡せないので読み戻す。

mod h264;
mod hevc;
mod mpeg2;
mod pool;
pub mod va;

use mediadecoder::es::{Codec, Format, Framing, Picture};

use crate::decode::each_front;
use crate::decode::{
    About, Decode, Decoded, DmaBuf, DmaBufPlane, Error, Held as Where, Readback, Surface,
};
use pool::Pool;
use va::{VAConfigID, VAContextID};

pub type Held = crate::decode::frames::Held<usize>;

#[derive(Debug)]
enum Front {
    Mpeg2(Box<mpeg2::Mpeg2>),
    H264(Box<h264::H264>),
    Hevc(Box<hevc::Hevc>),
}

#[derive(Debug)]
struct Session {
    config: VAConfigID,
    context: VAContextID,
    pool: Pool,
    format: Format,
    vpp: Option<Vpp>,
}

#[derive(Debug)]
struct Vpp {
    config: VAConfigID,
    context: VAContextID,
    algorithm: std::ffi::c_int,
    forward: usize,
    backward: usize,
    outputs: Vec<va::VASurfaceID>,
    next: usize,
    /// 表示順のフレーム。`cursor` が次に出すフレームで、その前は過去の参照。
    frames: std::collections::VecDeque<Held>,
    cursor: usize,
    field: u8,
}

const OUTPUTS: usize = 8;

#[derive(Debug)]
pub struct Vaapi {
    display: va::Display,
    profile: std::ffi::c_int,
    front: Front,
    session: Option<Session>,
    /// The format the surfaces were made for; a change needs a new pool.
    format: Format,
    said_read_back: bool,
    /// 指定されたデインターレース（`VAProcDeinterlacingType`）。`None` はウィーブだけ。
    deinterlace: Option<std::ffi::c_int>,
    /// レンダリングと違う GPU でデコードしている。フレームはメモリーで渡す。
    transfer: bool,
    device: String,
    ending: bool,
}

impl Vaapi {
    pub fn open_pick(
        codec: Codec,
        framing: Framing,
        device: &str,
        deinterlace: Option<crate::decode::HwDeinterlace>,
    ) -> Result<Self, Error> {
        let (display, transfer) = if device.is_empty() {
            (va::Display::open_any()?, crate::decode::memory_only())
        } else {
            let all = nodes();
            let node = named_nodes(&all)
                .into_iter()
                .find(|(_, name)| name == device)
                .map(|(node, _)| node)
                .ok_or_else(|| Error::Unavailable(format!("{device} が見つかりません")))?;
            (
                va::Display::open(&node)?,
                crate::decode::memory_only() || !draws(&node, &all),
            )
        };
        let mut decoder = Self::open_on(display, codec, framing)?;
        decoder.transfer = transfer;
        decoder.device = device.to_owned();
        if let Some(mode) = deinterlace {
            decoder.deinterlace = Some(decoder.algorithm(mode)?);
        }
        Ok(decoder)
    }

    /// 開くときにエラーにする（フレームが来てからでは何枚も捨てる）。
    fn algorithm(&self, mode: crate::decode::HwDeinterlace) -> Result<std::ffi::c_int, Error> {
        let wanted = match mode {
            crate::decode::HwDeinterlace::Bob => va::VA_PROC_DEINTERLACING_BOB,
            crate::decode::HwDeinterlace::Adaptive => va::VA_PROC_DEINTERLACING_MOTION_ADAPTIVE,
            crate::decode::HwDeinterlace::MotionCompensated => {
                va::VA_PROC_DEINTERLACING_MOTION_COMPENSATED
            }
        };
        if !self.display.has_video_proc() {
            return Err(Error::Unavailable(format!(
                "{} はビデオ処理（VAEntrypointVideoProc）に対応していません",
                self.display.vendor
            )));
        }
        let config = self.display.video_proc_config()?;
        let context = match self.display.context(config, 1920, 1088, &mut []) {
            Ok(context) => context,
            Err(error) => {
                self.display.destroy_config(config);
                return Err(error);
            }
        };
        let have = self.display.deinterlacers(context);
        self.display.destroy_context(context);
        self.display.destroy_config(config);
        if have.contains(&wanted) {
            Ok(wanted)
        } else {
            Err(Error::Unavailable(format!(
                "{} は {} のデインターレースに対応していません（対応している方式: {have:?}）",
                self.display.vendor,
                mode.name()
            )))
        }
    }

    /// Opens a decoder for a coding; nothing is allocated until a sequence header gives the size.
    pub fn open(codec: Codec, framing: Framing) -> Result<Self, Error> {
        Self::open_on(va::Display::open_any()?, codec, framing)
    }

    fn open_on(display: va::Display, codec: Codec, framing: Framing) -> Result<Self, Error> {
        if codec == Codec::Mpeg2
            && mpeg2_without_references(&display.vendor)
            && std::env::var_os("ZW_VAAPI_MPEG2").is_none()
        {
            return Err(Error::Unavailable(format!(
                "{} は MPEG-2 の参照をデコーダーへ渡さない（Mesa 26.2 の radeonsi の退行）。\
                 ZW_VAAPI_MPEG2=1 で試せます",
                display.vendor
            )));
        }
        // Main 10 first: a driver listing only Main 10 still decodes eight bit HEVC.
        let profile = profiles(codec)
            .iter()
            .copied()
            .find(|profile| display.has(*profile))
            .ok_or_else(|| {
                Error::Unavailable(format!(
                    "{} は {codec:?} をデコードできません",
                    display.vendor
                ))
            })?;
        let front = match codec {
            Codec::Mpeg2 => Front::Mpeg2(Box::default()),
            Codec::H264 => Front::H264(Box::new(h264::H264::new(framing))),
            Codec::Hevc => Front::Hevc(Box::new(hevc::Hevc::new(framing))),
        };
        Ok(Self {
            display,
            profile,
            front,
            session: None,
            format: Format::default(),
            said_read_back: false,
            deinterlace: None,
            transfer: false,
            device: String::new(),
            ending: false,
        })
    }

    /// 読み戻しに失敗したか（`vaDeriveImage` に対応しないドライバーがある）。
    pub fn read_back_refused(&self) -> bool {
        self.said_read_back
    }

    pub fn vendor(&self) -> &str {
        &self.display.vendor
    }

    fn configure(&mut self, format: Format) -> Result<(), Error> {
        if let Some(session) = &self.session
            && crate::decode::session_fits(session.format, format)
        {
            self.format = format;
            return Ok(());
        }
        self.teardown();
        crate::decode::openable(format)?;
        let wide = format.depth > 8;
        let profile = if wide && self.profile == va::VA_PROFILE_HEVC_MAIN {
            return Err(Error::Unsupported(
                "ten bit samples on an eight bit profile",
            ));
        } else {
            self.profile
        };
        let (rt_format, fourcc) = if wide {
            (va::VA_RT_FORMAT_YUV420_10, va::VA_FOURCC_P010)
        } else {
            (va::VA_RT_FORMAT_YUV420, va::VA_FOURCC_NV12)
        };
        let config = self.display.config(profile, rt_format)?;
        let width = format.width.next_multiple_of(16);
        let height = format.height.next_multiple_of(16);
        let vpp = match self.deinterlace {
            Some(algorithm) => match self.vpp(algorithm, rt_format, fourcc, width, height) {
                Ok(vpp) => Some(vpp),
                Err(error) => {
                    self.display.destroy_config(config);
                    return Err(error);
                }
            },
            None => None,
        };
        // VPP の参照として持つフレームの数だけサーフェスを足す。足さないとデコーダーの書き込み先がなくなる。
        let held_for_vpp = vpp.as_ref().map_or(0, |vpp| vpp.forward + vpp.backward + 2);
        let count = match &self.front {
            Front::Mpeg2(_) => 3,
            Front::H264(_) => 17,
            Front::Hevc(_) => 16,
        } + pool::SLACK
            + held_for_vpp;
        let surfaces = match self
            .display
            .surfaces(rt_format, fourcc, width, height, count)
        {
            Ok(surfaces) => surfaces,
            Err(error) => {
                if let Some(vpp) = vpp {
                    self.drop_vpp(vpp);
                }
                self.display.destroy_config(config);
                return Err(error);
            }
        };
        let mut pool = Pool::new(surfaces);
        let context = match self.display.context(config, width, height, pool.ids_mut()) {
            Ok(context) => context,
            Err(error) => {
                if let Some(vpp) = vpp {
                    self.drop_vpp(vpp);
                }
                self.display.destroy_surfaces(pool.ids_mut());
                self.display.destroy_config(config);
                return Err(error);
            }
        };
        self.session = Some(Session {
            config,
            context,
            pool,
            format,
            vpp,
        });
        self.format = format;
        Ok(())
    }

    fn vpp(
        &self,
        algorithm: std::ffi::c_int,
        rt_format: std::ffi::c_uint,
        fourcc: u32,
        width: u32,
        height: u32,
    ) -> Result<Vpp, Error> {
        let config = self.display.video_proc_config()?;
        let context = match self.display.context(config, width, height, &mut []) {
            Ok(context) => context,
            Err(error) => {
                self.display.destroy_config(config);
                return Err(error);
            }
        };
        let asked = self
            .display
            .parameters(
                context,
                va::VA_PROC_FILTER_PARAMETER_BUFFER_TYPE,
                &va::DeinterlacingParameters {
                    kind: va::VA_PROC_FILTER_DEINTERLACING,
                    algorithm,
                    ..va::DeinterlacingParameters::default()
                },
            )
            .and_then(|filter| {
                let answer = self.display.references(context, filter);
                self.display.destroy_buffer(filter);
                answer
            });
        let outputs = asked.and_then(|references| {
            self.display
                .surfaces(rt_format, fourcc, width, height, OUTPUTS)
                .map(|outputs| (references, outputs))
        });
        let ((forward, backward), outputs) = match outputs {
            Ok(made) => made,
            Err(error) => {
                self.display.destroy_context(context);
                self.display.destroy_config(config);
                return Err(error);
            }
        };
        if crate::decode::vaapi::say() {
            tracing::info!(
                "VA-API VPP: アルゴリズム {algorithm}、参照 過去 {forward}・未来 {backward}"
            );
        }
        Ok(Vpp {
            config,
            context,
            algorithm,
            forward: forward as usize,
            backward: backward as usize,
            outputs,
            next: 0,
            frames: std::collections::VecDeque::new(),
            cursor: 0,
            field: 0,
        })
    }

    fn drop_vpp(&self, mut vpp: Vpp) {
        self.display.destroy_context(vpp.context);
        self.display.destroy_surfaces(&mut vpp.outputs);
        self.display.destroy_config(vpp.config);
    }

    fn forget_frames(session: &mut Session) {
        if let Some(vpp) = session.vpp.as_mut() {
            for held in vpp.frames.drain(..) {
                session.pool.issued(held.slot);
            }
            vpp.cursor = 0;
            vpp.field = 0;
        }
    }

    fn teardown(&mut self) {
        let Some(mut session) = self.session.take() else {
            return;
        };
        Self::forget_frames(&mut session);
        if let Some(vpp) = session.vpp.take() {
            self.drop_vpp(vpp);
        }
        each_front!(&mut self.front, |front| front.reset(&mut session.pool));
        self.display.destroy_context(session.context);
        self.display.destroy_surfaces(session.pool.ids_mut());
        self.display.destroy_config(session.config);
    }

    /// The next picture as DMA-BUF descriptors, or read back on a GPU that does not draw.
    fn export(&mut self, held: Held) -> Option<Decoded> {
        let session = self.session.as_mut()?;
        let surface = session.pool.id(held.slot);
        let about = held.about(self.format);
        let handed = self.hand_over(surface, about);
        if let Some(session) = self.session.as_mut() {
            session.pool.issued(held.slot);
        }
        handed
    }

    fn hand_over(&mut self, surface: va::VASurfaceID, about: About) -> Option<Decoded> {
        if self.transfer {
            return match self.display.read_back(surface) {
                Ok(read) => frame_of(&read, about).map(Decoded::Planes),
                Err(why) => {
                    if !self.said_read_back {
                        self.said_read_back = true;
                        tracing::warn!("VA-API: 読み戻せない（GPU 間転送）: {why:?}");
                    }
                    None
                }
            };
        }
        self.export_surface(surface, about)
    }

    fn export_surface(&mut self, surface: va::VASurfaceID, about: About) -> Option<Decoded> {
        // Nothing may be exported until the driver has finished writing the surface.
        if let Err(error) = self.display.sync(surface) {
            tracing::warn!("VA-API: vaSyncSurface に失敗: {error:?}");
        }
        let descriptor = match self.display.export(surface) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                tracing::warn!("VA-API: DMA-BUF をエクスポートできない: {error:?}");
                return None;
            }
        };
        let mut planes = Vec::new();
        let mut modifier = 0;
        for layer in 0..descriptor.num_layers.min(4) as usize {
            let layer = descriptor.layers[layer];
            let object = descriptor.objects[(layer.object_index[0] as usize).min(3)];
            modifier = object.drm_format_modifier;
            // Layers may share an object, so each layer takes its own duplicate.
            let Some(fd) = va::duplicate(object.fd) else {
                continue;
            };
            planes.push(DmaBufPlane {
                fd,
                offset: layer.offset[0],
                stride: layer.pitch[0],
                format: layer.drm_format,
            });
        }
        for object in 0..descriptor.num_objects.min(4) as usize {
            va::discard(descriptor.objects[object].fd);
        }
        if planes.is_empty() {
            return None;
        }
        Some(Decoded::Surface(Surface {
            about,
            held: Where::DmaBuf(DmaBuf {
                width: self.format.width,
                height: self.format.height,
                modifier,
                planes,
                lease: None,
            }),
        }))
    }

    pub fn read_back(&mut self) -> Option<(About, va::Readback)> {
        let held = self.next_held()?;
        let session = self.session.as_mut()?;
        let surface = session.pool.id(held.slot);
        // 読めなかった理由を一度だけログに出す。黙って `None` だとフレームが出ないとしか見えない。
        let read = match self.display.read_back(surface) {
            Ok(read) => Some(read),
            Err(why) => {
                if !self.said_read_back {
                    self.said_read_back = true;
                    tracing::warn!("VA-API: 読み戻せない: {why:?}");
                }
                None
            }
        };
        if let Some(session) = self.session.as_mut() {
            session.pool.issued(held.slot);
        }
        Some((held.about(self.format), read?))
    }

    fn next_held(&mut self) -> Option<Held> {
        each_front!(&mut self.front, |front| front.take())
    }

    /// 未来の参照が要る方式ではその数だけ先のフレームを待つ（flush の後は待たない）。
    fn take_deinterlaced(&mut self) -> Option<Decoded> {
        loop {
            let session = self.session.as_ref()?;
            let vpp = session.vpp.as_ref()?;
            if vpp.frames.len() > vpp.cursor + vpp.backward {
                break;
            }
            let Some(held) = self.next_held() else {
                break;
            };
            self.session.as_mut()?.vpp.as_mut()?.frames.push_back(held);
        }
        let ending = self.ending;
        let format = self.format;
        let session = self.session.as_mut()?;
        let vpp = session.vpp.as_mut()?;
        if vpp.frames.len() <= vpp.cursor {
            return None;
        }
        if vpp.frames.len() <= vpp.cursor + vpp.backward && !ending {
            return None;
        }
        let current = vpp.frames[vpp.cursor];
        let mut forward: Vec<va::VASurfaceID> = (1..=vpp.forward)
            .filter_map(|back| vpp.cursor.checked_sub(back))
            .map(|at| session.pool.id(vpp.frames[at].slot))
            .collect();
        let mut backward: Vec<va::VASurfaceID> = (1..=vpp.backward)
            .map(|ahead| vpp.cursor + ahead)
            .filter(|at| *at < vpp.frames.len())
            .map(|at| session.pool.id(vpp.frames[at].slot))
            .collect();
        let output = vpp.outputs[vpp.next];
        vpp.next = (vpp.next + 1) % vpp.outputs.len();
        let second = vpp.field == 1;
        let surface = session.pool.id(current.slot);
        let processed = process_field(
            &self.display,
            vpp,
            surface,
            output,
            &current,
            second,
            &mut forward,
            &mut backward,
        );
        if second {
            vpp.field = 0;
            vpp.cursor += 1;
            while vpp.cursor > vpp.forward {
                if let Some(old) = vpp.frames.pop_front() {
                    session.pool.issued(old.slot);
                }
                vpp.cursor -= 1;
            }
        } else {
            vpp.field = 1;
        }
        if let Err(error) = processed {
            tracing::warn!("VA-API VPP: {error:?}");
            return None;
        }
        self.hand_over(output, woven(current.about(format)))
    }
}

/// プログレッシブのフレームはフィルターを通さずにコピーする（フィールドレートに揃えるため二回）。
#[allow(clippy::too_many_arguments)]
fn process_field(
    display: &va::Display,
    vpp: &Vpp,
    surface: va::VASurfaceID,
    output: va::VASurfaceID,
    current: &Held,
    second: bool,
    forward: &mut [va::VASurfaceID],
    backward: &mut [va::VASurfaceID],
) -> Result<(), Error> {
    let mut filters: Vec<va::VABufferID> = Vec::new();
    if !current.progressive {
        let bottom_first = !current.top_field_first;
        let bottom = bottom_first != second;
        let mut flags = 0;
        if bottom_first {
            flags |= va::VA_DEINTERLACING_BOTTOM_FIELD_FIRST;
        }
        if bottom {
            flags |= va::VA_DEINTERLACING_BOTTOM_FIELD;
        }
        filters.push(display.parameters(
            vpp.context,
            va::VA_PROC_FILTER_PARAMETER_BUFFER_TYPE,
            &va::DeinterlacingParameters {
                kind: va::VA_PROC_FILTER_DEINTERLACING,
                algorithm: vpp.algorithm,
                flags,
                ..va::DeinterlacingParameters::default()
            },
        )?);
    }
    let parameters = va::PipelineParameters {
        surface,
        filters: if filters.is_empty() {
            std::ptr::null_mut()
        } else {
            filters.as_mut_ptr()
        },
        num_filters: filters.len() as u32,
        forward_references: if forward.is_empty() || filters.is_empty() {
            std::ptr::null_mut()
        } else {
            forward.as_mut_ptr()
        },
        num_forward_references: if filters.is_empty() {
            0
        } else {
            forward.len() as u32
        },
        backward_references: if backward.is_empty() || filters.is_empty() {
            std::ptr::null_mut()
        } else {
            backward.as_mut_ptr()
        },
        num_backward_references: if filters.is_empty() {
            0
        } else {
            backward.len() as u32
        },
        ..va::PipelineParameters::default()
    };
    // 引数のバッファが指すフィルターと参照の配列は、`vaRenderPicture` の間生きている。
    let made = display.parameters(
        vpp.context,
        va::VA_PROC_PIPELINE_PARAMETER_BUFFER_TYPE,
        &parameters,
    );
    let result =
        made.and_then(|pipeline| display.process(vpp.context, output, &mut vec![pipeline]));
    for filter in filters {
        display.destroy_buffer(filter);
    }
    result
}

fn woven(about: About) -> About {
    About {
        structure: mediadecoder::es::Structure::Frame,
        progressive: true,
        repeat_first_field: false,
        ..about
    }
}

fn algorithm_name(algorithm: std::ffi::c_int) -> &'static str {
    match algorithm {
        va::VA_PROC_DEINTERLACING_BOB => "Bob",
        va::VA_PROC_DEINTERLACING_MOTION_ADAPTIVE => "適応型",
        va::VA_PROC_DEINTERLACING_MOTION_COMPENSATED => "動き補償",
        _ => "?",
    }
}

fn profiles(codec: Codec) -> &'static [std::ffi::c_int] {
    match codec {
        Codec::Mpeg2 => &[va::VA_PROFILE_MPEG2_MAIN],
        Codec::H264 => &[va::VA_PROFILE_H264_HIGH],
        Codec::Hevc => &[va::VA_PROFILE_HEVC_MAIN10, va::VA_PROFILE_HEVC_MAIN],
    }
}

/// 読み戻した `NV12` / `P010` を 3 プレーンにする。10 ビットは 2 バイトの下位へ詰め直す。
fn frame_of(read: &va::Readback, about: About) -> Option<mediadecoder::video::Frame> {
    use mediadecoder::video::Plane;
    let (luma, luma_pitch, _) = read.planes.first()?;
    let (chroma, chroma_pitch, _) = read.planes.get(1)?;
    let width = about.format.width as usize;
    let height = about.format.height as usize;
    let wide = about.format.depth > 8;
    let size = if wide { 2 } else { 1 };
    let half_w = width.div_ceil(2);
    let half_h = height.div_ceil(2);
    let sample = |bytes: &[u8], at: usize| -> Option<[u8; 2]> {
        let value = u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) >> 6;
        Some(value.to_le_bytes())
    };
    let mut y = Vec::with_capacity(width * height * size);
    for row in 0..height {
        let at = row * luma_pitch;
        if wide {
            for x in 0..width {
                y.extend_from_slice(&sample(luma, at + x * 2)?);
            }
        } else {
            y.extend_from_slice(luma.get(at..at + width)?);
        }
    }
    let mut blue = Vec::with_capacity(half_w * half_h * size);
    let mut red = Vec::with_capacity(half_w * half_h * size);
    for row in 0..half_h {
        let at = row * chroma_pitch;
        for x in 0..half_w {
            if wide {
                blue.extend_from_slice(&sample(chroma, at + x * 4)?);
                red.extend_from_slice(&sample(chroma, at + x * 4 + 2)?);
            } else {
                blue.push(*chroma.get(at + x * 2)?);
                red.push(*chroma.get(at + x * 2 + 1)?);
            }
        }
    }
    let plane = |data, width, height| Plane {
        data,
        stride: width * size,
        width,
        height,
    };
    Some(mediadecoder::video::Frame {
        tag: about.tag,
        format: about.format,
        planes: [
            plane(y, width, height),
            plane(blue, half_w, half_h),
            plane(red, half_w, half_h),
        ],
        structure: about.structure,
        top_field_first: about.top_field_first,
        repeat_first_field: about.repeat_first_field,
        progressive: about.progressive,
    })
}

pub(crate) fn say() -> bool {
    SAY.on()
}

static SAY: crate::trace::Trace = crate::trace::Trace::of("vaapi");

/// レンダリングに使っているレンダーノード（`st_rdev`）。レンダラーが設定する。
static RENDER_NODE: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

pub fn adopt_render_node(major: u32, minor: u32) {
    if let Ok(mut slot) = RENDER_NODE.lock() {
        *slot = Some(makedev(major, minor));
    }
}

fn makedev(major: u32, minor: u32) -> u64 {
    let (major, minor) = (u64::from(major), u64::from(minor));
    ((major & 0xffff_f000) << 32)
        | ((major & 0x0000_0fff) << 8)
        | ((minor & 0xffff_ff00) << 12)
        | (minor & 0x0000_00ff)
}

fn nodes() -> Vec<String> {
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
    nodes
}

fn pci_of(node: &str) -> Option<(u16, u16)> {
    let base = node.rsplit('/').next()?;
    let read = |what: &str| -> Option<u16> {
        let text = std::fs::read_to_string(format!("/sys/class/drm/{base}/device/{what}")).ok()?;
        u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
    };
    Some((read("vendor")?, read("device")?))
}

pub fn device_name(node: &str) -> Option<String> {
    let (vendor, device) = pci_of(node)?;
    let short = match vendor {
        0x8086 => String::from("Intel"),
        0x1002 => String::from("AMD"),
        0x10de => String::from("NVIDIA"),
        other => format!("{other:04x}"),
    };
    let model = pci_name(vendor, device);
    Some(match model {
        Some(model) if model.starts_with(&short) => model,
        Some(model) => format!("{short} {model}"),
        None => format!("{short} {vendor:04x}:{device:04x}"),
    })
}

fn pci_name(vendor: u16, device: u16) -> Option<String> {
    let text = [
        "/usr/share/misc/pci.ids",
        "/usr/share/hwdata/pci.ids",
        "/usr/share/pci.ids",
    ]
    .iter()
    .find_map(|path| std::fs::read_to_string(path).ok())?;
    parse_pci_ids(&text, vendor, device)
}

fn parse_pci_ids(text: &str, vendor: u16, device: u16) -> Option<String> {
    let vendor = format!("{vendor:04x}");
    let device = format!("\t{device:04x}");
    let mut inside = false;
    for line in text.lines() {
        if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
            inside = line.starts_with(&vendor);
            continue;
        }
        if inside && line.starts_with(&device) {
            let name = line[device.len()..].trim();
            let name = match (name.rfind('['), name.rfind(']')) {
                (Some(open), Some(close)) if open < close => &name[open + 1..close],
                _ => name,
            };
            return Some(name.to_owned());
        }
    }
    None
}

/// レンダリングしている GPU のレンダーノードとベンダー。設定されていなければノードが一つのときだけ。
pub fn drawing_node() -> Option<(String, u16)> {
    let all = nodes();
    let node = all.iter().find(|node| draws(node, &all))?;
    Some((node.clone(), pci_of(node)?.0))
}

/// レンダーノードと GPU の名前の組（二つ目からはノード名を添える）。NVIDIA のデバイスは除く。
pub fn render_nodes() -> Vec<(String, String)> {
    named_nodes(&nodes())
}

fn named_nodes(all: &[String]) -> Vec<(String, String)> {
    let mut named: Vec<(String, String)> = Vec::new();
    for node in all {
        if pci_of(node).is_some_and(|(vendor, _)| vendor == 0x10de) {
            continue;
        }
        let Some(mut name) = device_name(node) else {
            continue;
        };
        if named.iter().any(|(_, seen)| *seen == name) {
            let base = node.rsplit('/').next().unwrap_or(node);
            name = format!("{name} ({base})");
        }
        named.push((node.clone(), name));
    }
    named
}

fn draws(node: &str, all: &[String]) -> bool {
    use std::os::unix::fs::MetadataExt;
    let adopted = RENDER_NODE.lock().ok().and_then(|slot| *slot);
    match adopted {
        Some(render) => std::fs::metadata(node).is_ok_and(|meta| meta.rdev() == render),
        None => all.len() == 1,
    }
}

/// 新しい radeonsi は MPEG-2 の参照のサーフェス番号を埋めず映像が緑になるので、自前のデコーダーへ回す。
fn mpeg2_without_references(vendor: &str) -> bool {
    if !vendor.contains("radeonsi") {
        return false;
    }
    let Some(version) = vendor
        .split("Mesa Gallium driver ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
    else {
        return false;
    };
    let mut parts = version
        .split(['.', '-'])
        .map(|part| part.parse::<u32>().ok());
    let (Some(Some(major)), Some(Some(minor))) = (parts.next(), parts.next()) else {
        return false;
    };
    (major, minor) >= (26, 2)
}

impl Drop for Vaapi {
    fn drop(&mut self) {
        self.teardown();
    }
}

impl Decode for Vaapi {
    fn decode(
        &mut self,
        tag: u64,
        picture: &Picture,
        format: Format,
        data: &[u8],
    ) -> Result<(), Error> {
        self.ending = false;
        self.configure(format)?;
        let Some(session) = self.session.as_mut() else {
            return Err(Error::Unavailable(String::from("サーフェスがありません")));
        };
        each_front!(&mut self.front, |front| front.decode(
            &self.display,
            session.context,
            &mut session.pool,
            tag,
            picture,
            format,
            data,
        ))
    }

    fn take(&mut self) -> Option<Decoded> {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.vpp.is_some())
        {
            return self.take_deinterlaced();
        }
        let held = self.next_held()?;
        self.export(held)
    }

    fn read_back(&mut self) -> Option<Readback> {
        let (about, read) = Vaapi::read_back(self)?;
        Some(Readback {
            about,
            planes: read.planes,
        })
    }

    fn name(&self) -> String {
        let mut name = if self.device.is_empty() {
            format!("VA-API ({})", self.vendor())
        } else {
            format!("VA-API ({} — {})", self.device, self.vendor())
        };
        if self.transfer {
            name.push_str("・GPU 間転送");
        }
        if let Some(algorithm) = self.deinterlace {
            name.push_str(&format!("・デインターレース {}", algorithm_name(algorithm)));
        }
        name
    }

    fn deinterlaces(&self) -> bool {
        self.deinterlace.is_some()
    }

    fn pictures_per_frame(&self) -> u32 {
        if self.deinterlace.is_some() { 2 } else { 1 }
    }

    fn flush(&mut self) {
        self.ending = true;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        each_front!(&mut self.front, |front| front.flush(&mut session.pool))
    }

    fn reset(&mut self) {
        self.ending = false;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        Self::forget_frames(session);
        each_front!(&mut self.front, |front| front.reset(&mut session.pool));
        session.pool.forget_recent();
    }
}

/// このコーデックを VA-API でデコードできるデバイス（[`crate::decode::offers`]）。NVIDIA のデバイスは除く。
pub fn offers(codec: Codec) -> Vec<crate::decode::Offer> {
    let all = nodes();
    let mut offers = Vec::new();
    for (node, name) in named_nodes(&all) {
        let Ok(display) = va::Display::open(&node) else {
            continue;
        };
        if !profiles(codec).iter().any(|profile| display.has(*profile)) {
            continue;
        }
        if codec == Codec::Mpeg2 && mpeg2_without_references(&display.vendor) {
            continue;
        }
        offers.push(crate::decode::Offer {
            device: name,
            ways: vec![crate::decode::Way::Vaapi],
            gpu: true,
            draws: draws(&node, &all),
        });
    }
    offers
}

#[cfg(test)]
mod tests {
    use super::{makedev, mpeg2_without_references, parse_pci_ids};

    #[test]
    fn a_model_is_read_out_of_pci_ids() {
        let text = "# comment\n8086  Intel Corporation\n\t3e92  CoffeeLake-S GT2 [UHD Graphics 630]\n\t\t1028 0869  Vostro\n10de  NVIDIA Corporation\n\t3e92  Not this one\n1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t67ff  Baffin [Radeon RX 550 640SP / RX 560/560X]\n";
        assert_eq!(
            parse_pci_ids(text, 0x8086, 0x3e92).as_deref(),
            Some("UHD Graphics 630")
        );
        assert_eq!(
            parse_pci_ids(text, 0x1002, 0x67ff).as_deref(),
            Some("Radeon RX 550 640SP / RX 560/560X")
        );
        assert_eq!(parse_pci_ids(text, 0x8086, 0x1234), None);
    }

    #[test]
    fn a_device_number_is_glibcs() {
        assert_eq!(makedev(226, 128), 0xe280);
    }

    #[test]
    fn only_radeonsi_from_26_2_is_refused_mpeg2() {
        let radeon = |version: &str| {
            format!(
                "Mesa Gallium driver {version} for AMD Radeon Graphics (radeonsi, renoir, ACO, DRM 3.64)"
            )
        };
        assert!(mpeg2_without_references(&radeon("26.2.3-arch1.1")));
        assert!(mpeg2_without_references(&radeon("27.0.0")));
        assert!(!mpeg2_without_references(&radeon("26.1.4")));
        assert!(!mpeg2_without_references(&radeon("25.0.7")));
        assert!(!mpeg2_without_references(
            "Intel iHD driver for Intel(R) Gen Graphics - 24.1.0"
        ));
    }
}
