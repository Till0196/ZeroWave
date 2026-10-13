// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! The device: ioctls, queues, buffers and requests. Stateless order:
//! `S_EXT_CTRLS(request)`, `QBUF(REQUEST_FD)`, `MEDIA_REQUEST_IOC_QUEUE`.

use std::ffi::{c_int, c_void};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;

use crate::decode::Error;

use super::ctrl::{Ctl, Kind, Naming};
use super::uapi::{self, *};

fn failed(what: &str, error: io::Error) -> Error {
    Error::Unavailable(format!("{what}: {error}"))
}

/// `arg` must point at a structure of exactly the size `request` encodes.
unsafe fn ioctl<T>(fd: RawFd, request: std::ffi::c_ulong, arg: &mut T) -> io::Result<()> {
    // SAFETY: the caller's contract; `arg` is a live `&mut T` for the call.
    let out = unsafe { ioctl_raw(fd, request, arg as *mut T as *mut c_void) };
    if out < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// 引数がポインターを持たない要求を、要求ごとの安全な関数にする。
macro_rules! plain_ioctls {
    ($($name:ident($request:expr, $arg:ty);)*) => {$(
        fn $name(fd: RawFd, arg: &mut $arg) -> io::Result<()> {
            // SAFETY: 要求番号は `$arg` のサイズから作ってあり、`$arg` はドライバーが辿るポインターを持たない。
            unsafe { ioctl(fd, $request, arg) }
        }
    )*};
}

plain_ioctls! {
    query_cap(VIDIOC_QUERYCAP, Capability);
    enum_fmt(VIDIOC_ENUM_FMT, FmtDesc);
    g_fmt(VIDIOC_G_FMT, uapi::Format);
    s_fmt(VIDIOC_S_FMT, uapi::Format);
    reqbufs(VIDIOC_REQBUFS, RequestBuffers);
    create_bufs(uapi::VIDIOC_CREATE_BUFS, uapi::CreateBuffers);
    expbuf(VIDIOC_EXPBUF, ExportBuffer);
    streamon(VIDIOC_STREAMON, c_int);
    streamoff(VIDIOC_STREAMOFF, c_int);
    query_ext_ctrl(VIDIOC_QUERY_EXT_CTRL, QueryExtCtrl);
    decoder_cmd(VIDIOC_DECODER_CMD, DecoderCmd);
    subscribe_event(VIDIOC_SUBSCRIBE_EVENT, EventSubscription);
    dqevent(VIDIOC_DQEVENT, Event);
    g_ctrl(VIDIOC_G_CTRL, Control);
    g_selection(VIDIOC_G_SELECTION, Selection);
    request_alloc(MEDIA_IOC_REQUEST_ALLOC, i32);
}

/// 引数の無い要求。
#[derive(Debug, Clone, Copy)]
enum Bare {
    RequestQueue,
    RequestReinit,
}

fn ioctl_bare(fd: RawFd, request: Bare) -> io::Result<()> {
    let request = match request {
        Bare::RequestQueue => MEDIA_REQUEST_IOC_QUEUE,
        Bare::RequestReinit => MEDIA_REQUEST_IOC_REINIT,
    };
    // SAFETY: どちらも `_IO(...)` で、引数を読まない。
    let out = unsafe { ioctl_raw(fd, request, 0usize) };
    if out < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `v4l2_buffer` を渡す要求。
#[derive(Debug, Clone, Copy)]
enum BufferOp {
    Query,
    Queue,
    Dequeue,
}

/// 読み書きする MMAP のバッファ。
#[derive(Debug)]
pub struct Mapping(crate::decode::mmap::Mapping);

impl Mapping {
    fn new(fd: RawFd, offset: u32, length: usize) -> io::Result<Self> {
        crate::decode::mmap::Mapping::new(fd, offset as usize, length, true).map(Self)
    }

    pub fn read(&self) -> &[u8] {
        self.0.bytes()
    }

    pub fn write(&mut self, bytes: &[u8]) -> usize {
        self.write_at(0, bytes)
    }

    fn write_at(&mut self, at: usize, bytes: &[u8]) -> usize {
        let Some(into) = self.0.bytes_mut().get_mut(at..) else {
            return 0;
        };
        let count = bytes.len().min(into.len());
        into[..count].copy_from_slice(&bytes[..count]);
        count
    }

    pub fn fill(
        &mut self,
        prefix: &[u8],
        data: &[u8],
        pieces: &[std::ops::Range<usize>],
        zeroes: usize,
    ) -> usize {
        let mut at = 0;
        for piece in pieces {
            let Some(bytes) = data.get(piece.clone()) else {
                continue;
            };
            at += self.write_at(at, prefix);
            at += self.write_at(at, bytes);
        }
        at += self.write_at(at, &[0; 8][..zeroes.min(8)]);
        at
    }
}

/// Callers always name the multi-planar queues; a single-planar driver is
/// translated here so nothing above has to know.
#[derive(Debug)]
pub struct Device {
    file: File,
    pub path: String,
    pub driver: String,
    pub card: String,
    pub single_planar: bool,
}

impl Device {
    /// Non-blocking: a stateful decoder holds pictures back, so queues are polled.
    pub fn open(path: &str) -> io::Result<Self> {
        let file = File::options()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(path)?;
        let mut cap = Capability {
            driver: [0; 16],
            card: [0; 32],
            bus_info: [0; 32],
            version: 0,
            capabilities: 0,
            device_caps: 0,
            reserved: [0; 3],
        };
        query_cap(file.as_raw_fd(), &mut cap)?;
        let caps = if cap.capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        if caps & (V4L2_CAP_VIDEO_M2M_MPLANE | V4L2_CAP_VIDEO_M2M) == 0
            || caps & V4L2_CAP_STREAMING == 0
        {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        Ok(Self {
            file,
            path: path.to_owned(),
            driver: text(&cap.driver),
            card: text(&cap.card),
            single_planar: caps & V4L2_CAP_VIDEO_M2M_MPLANE == 0,
        })
    }

    fn queue_type(&self, kind: u32) -> u32 {
        match kind {
            V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE if self.single_planar => V4L2_BUF_TYPE_VIDEO_OUTPUT,
            V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE if self.single_planar => V4L2_BUF_TYPE_VIDEO_CAPTURE,
            other => other,
        }
    }

    fn get_format(&self, kind: u32) -> Result<uapi::Format, Error> {
        let mut format = uapi::Format::new(self.queue_type(kind));
        g_fmt(self.fd(), &mut format).map_err(|error| failed("VIDIOC_G_FMT", error))?;
        if self.single_planar {
            format.pix_mp = format.pix().to_mplane();
        }
        Ok(format)
    }

    fn put_format(&self, mut format: uapi::Format) -> Result<PixFormatMplane, Error> {
        format.kind = self.queue_type(format.kind);
        if self.single_planar {
            let pix = PixFormat::of_mplane(&format.pix_mp);
            format.pix_mp = PixFormatMplane::default();
            format.set_pix(pix);
        }
        s_fmt(self.fd(), &mut format).map_err(|error| failed("VIDIOC_S_FMT", error))?;
        Ok(if self.single_planar {
            format.pix().to_mplane()
        } else {
            format.pix_mp
        })
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub fn formats(&self, kind: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for index in 0..64 {
            let mut desc = FmtDesc {
                index,
                kind: self.queue_type(kind),
                flags: 0,
                description: [0; 32],
                pixelformat: 0,
                mbus_code: 0,
                reserved: [0; 3],
            };
            if enum_fmt(self.fd(), &mut desc).is_err() {
                break;
            }
            out.push(desc.pixelformat);
        }
        out
    }

    pub fn set_format(
        &self,
        kind: u32,
        pixelformat: u32,
        width: u32,
        height: u32,
        sizeimage: u32,
        planes: u8,
    ) -> Result<PixFormatMplane, Error> {
        let mut format = uapi::Format::new(kind);
        format.pix_mp.width = width;
        format.pix_mp.height = height;
        format.pix_mp.pixelformat = pixelformat;
        format.pix_mp.field = V4L2_FIELD_NONE;
        format.pix_mp.num_planes = planes;
        format.pix_mp.plane_fmt[0].sizeimage = sizeimage;
        self.put_format(format)
    }

    pub fn format(&self, kind: u32) -> Result<PixFormatMplane, Error> {
        Ok(self.get_format(kind)?.pix_mp)
    }

    pub fn request_buffers(&self, kind: u32, count: u32) -> Result<(u32, u32), Error> {
        self.request_buffers_of(kind, count, V4L2_MEMORY_MMAP)
    }

    pub fn request_buffers_of(
        &self,
        kind: u32,
        count: u32,
        memory: u32,
    ) -> Result<(u32, u32), Error> {
        let mut request = RequestBuffers {
            count,
            kind: self.queue_type(kind),
            memory,
            ..RequestBuffers::default()
        };
        reqbufs(self.fd(), &mut request).map_err(|error| failed("VIDIOC_REQBUFS", error))?;
        Ok((request.count, request.capabilities))
    }

    /// 動いているキューに現在のフォーマットで追加する。返るのは（最初のインデックス、追加できた本数）。
    pub fn create_buffers(&self, kind: u32, count: u32) -> Result<(u32, u32), Error> {
        // シングルプレーンへの変換はせず、ドライバーの形式のまま渡す。
        let mut format = uapi::Format::new(self.queue_type(kind));
        g_fmt(self.fd(), &mut format).map_err(|error| failed("VIDIOC_G_FMT", error))?;
        let mut create = uapi::CreateBuffers::new(count, V4L2_MEMORY_MMAP, format);
        create_bufs(self.fd(), &mut create).map_err(|error| failed("VIDIOC_CREATE_BUFS", error))?;
        Ok((create.index, create.count))
    }

    pub fn query_buffer(&self, kind: u32, index: u32, plane: usize) -> Result<(u32, u32), Error> {
        if self.single_planar {
            if plane != 0 {
                return Err(Error::Unavailable(String::from(
                    "a single-planar buffer has only plane 0",
                )));
            }
            let mut buffer = Buffer::new(self.queue_type(kind), index);
            let mut planes = [Plane::default(); VIDEO_MAX_PLANES];
            self.buffer_ioctl(BufferOp::Query, &mut buffer, &mut planes)
                .map_err(|error| failed("VIDIOC_QUERYBUF", error))?;
            // SAFETY: a single-planar MMAP buffer reports `m.offset`.
            return Ok((unsafe { buffer.m.offset }, buffer.length));
        }
        let mut planes = [Plane::default(); VIDEO_MAX_PLANES];
        let mut buffer = Buffer::new(kind, index);
        buffer.length = VIDEO_MAX_PLANES as u32;
        self.buffer_ioctl(BufferOp::Query, &mut buffer, &mut planes)
            .map_err(|error| failed("VIDIOC_QUERYBUF", error))?;
        // SAFETY: the driver filled in a plain union of integers.
        let offset = unsafe { planes[plane].m.mem_offset };
        Ok((offset, planes[plane].length))
    }

    pub fn map(&self, kind: u32, index: u32, plane: usize) -> Result<Mapping, Error> {
        let (offset, length) = self.query_buffer(kind, index, plane)?;
        Mapping::new(self.fd(), offset, length as usize).map_err(|error| failed("mmap", error))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn queue_dmabuf(
        &self,
        kind: u32,
        index: u32,
        fd: RawFd,
        bytes: u32,
        ts: u64,
        field: u32,
    ) -> Result<(), Error> {
        let mut planes = [Plane::default(); VIDEO_MAX_PLANES];
        planes[0].bytesused = bytes;
        planes[0].length = bytes;
        planes[0].m = PlaneMemory { fd };
        let mut buffer = Buffer::new(self.queue_type(kind), index);
        buffer.memory = V4L2_MEMORY_DMABUF;
        buffer.bytesused = bytes;
        buffer.timestamp = Timeval::from_ns(ts);
        buffer.field = field;
        if self.single_planar {
            buffer.length = bytes;
            buffer.m = BufferMemory { fd };
        } else {
            buffer.length = 1;
        }
        self.buffer_ioctl(BufferOp::Queue, &mut buffer, &mut planes)
            .map_err(|error| failed("VIDIOC_QBUF", error))
    }

    pub fn queue(&self, what: &Enqueue) -> Result<(), Error> {
        let mut planes = [Plane::default(); VIDEO_MAX_PLANES];
        planes[0].bytesused = what.bytes;
        let mut buffer = Buffer::new(self.queue_type(what.kind), what.index);
        buffer.bytesused = what.bytes;
        buffer.timestamp = Timeval::from_ns(what.ts);
        buffer.field = what.field;
        if !self.single_planar {
            buffer.length = what.planes_used.clamp(1, VIDEO_MAX_PLANES as u32);
        }
        if what.hold {
            buffer.flags |= V4L2_BUF_FLAG_M2M_HOLD_CAPTURE_BUF;
        }
        if let Some(fd) = what.request {
            buffer.flags |= V4L2_BUF_FLAG_REQUEST_FD;
            buffer.request_fd = fd;
        }
        self.buffer_ioctl(BufferOp::Queue, &mut buffer, &mut planes)
            .map_err(|error| failed("VIDIOC_QBUF", error))
    }

    /// マルチプレーンなら `planes` を `m.planes` に繋ぎ、`length` を `planes` の数までに収めて呼ぶ。
    /// シングルプレーンの `m` は整数（MMAP のオフセットか DMA-BUF のファイル記述子）だけを許す。
    fn buffer_ioctl(
        &self,
        op: BufferOp,
        buffer: &mut Buffer,
        planes: &mut [Plane; VIDEO_MAX_PLANES],
    ) -> io::Result<()> {
        if buffer.memory != V4L2_MEMORY_MMAP && buffer.memory != V4L2_MEMORY_DMABUF {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        if !self.single_planar {
            buffer.length = buffer.length.min(VIDEO_MAX_PLANES as u32);
            buffer.m = BufferMemory {
                planes: planes.as_mut_ptr(),
            };
        }
        let request = match op {
            BufferOp::Query => VIDIOC_QUERYBUF,
            BufferOp::Queue => VIDIOC_QBUF,
            BufferOp::Dequeue => VIDIOC_DQBUF,
        };
        // SAFETY: 要求番号は `Buffer` のサイズから作ってある。マルチプレーンの `m.planes` は呼び出しの間
        // 生きる `length` 個の配列を指し、シングルプレーンの `m` はポインターでない。
        unsafe { ioctl(self.fd(), request, buffer) }
    }

    pub fn wait(&self, milliseconds: c_int) -> Poll {
        let mut watch = PollFd {
            fd: self.fd(),
            events: POLLIN | POLLOUT | POLLPRI,
            revents: 0,
        };
        // SAFETY: `watch` is one live `pollfd`.
        let out = unsafe { poll_raw(&mut watch, 1, milliseconds) };
        if out <= 0 {
            return Poll::default();
        }
        Poll {
            capture: watch.revents & POLLIN != 0,
            output: watch.revents & POLLOUT != 0,
            event: watch.revents & POLLPRI != 0,
        }
    }

    /// `EAGAIN` (empty) and `EPIPE` (drain finished) are not faults.
    pub fn dequeue_now(&self, kind: u32) -> Result<Option<Dequeued>, Error> {
        self.dequeue_now_of(kind, V4L2_MEMORY_MMAP)
    }

    pub fn dequeue_now_of(&self, kind: u32, memory: u32) -> Result<Option<Dequeued>, Error> {
        match self.dequeue_raw_of(kind, memory) {
            Ok(done) => Ok(Some(done)),
            Err(error) if matches!(error.raw_os_error(), Some(EAGAIN) | Some(EPIPE)) => Ok(None),
            Err(error) => Err(failed("VIDIOC_DQBUF", error)),
        }
    }

    fn dequeue_raw_of(&self, kind: u32, memory: u32) -> io::Result<Dequeued> {
        let mut planes = [Plane::default(); VIDEO_MAX_PLANES];
        let mut buffer = Buffer::new(self.queue_type(kind), 0);
        buffer.memory = memory;
        if !self.single_planar {
            buffer.length = VIDEO_MAX_PLANES as u32;
        }
        self.buffer_ioctl(BufferOp::Dequeue, &mut buffer, &mut planes)?;
        Ok(Dequeued {
            index: buffer.index as usize,
            ts: buffer.timestamp.to_ns(),
            error: buffer.flags & V4L2_BUF_FLAG_ERROR != 0,
            last: buffer.flags & V4L2_BUF_FLAG_LAST != 0,
            bytes: if self.single_planar {
                buffer.bytesused
            } else {
                planes[0].bytesused
            },
            field: buffer.field,
        })
    }

    pub fn export(&self, kind: u32, index: u32, plane: u32) -> Result<OwnedFd, Error> {
        let mut export = ExportBuffer {
            kind: self.queue_type(kind),
            index,
            plane,
            flags: 0o2000000,
            fd: -1,
            reserved: [0; 11],
        };
        expbuf(self.fd(), &mut export).map_err(|error| failed("VIDIOC_EXPBUF", error))?;
        if export.fd < 0 {
            return Err(Error::Unavailable(String::from(
                "VIDIOC_EXPBUF gave no descriptor",
            )));
        }
        // SAFETY: a fresh descriptor owned by this process alone.
        Ok(unsafe { OwnedFd::from_raw_fd(export.fd) })
    }

    pub fn stream_on(&self, kind: u32) -> Result<(), Error> {
        let mut kind = self.queue_type(kind) as c_int;
        streamon(self.fd(), &mut kind).map_err(|error| failed("VIDIOC_STREAMON", error))
    }

    pub fn stream_off(&self, kind: u32) {
        let mut kind = self.queue_type(kind) as c_int;
        // 失敗は無視する（キューを解放するところ）。
        let _ = streamoff(self.fd(), &mut kind);
    }

    pub fn control_size(&self, id: u32) -> Option<u32> {
        let mut query = QueryExtCtrl {
            id,
            ..QueryExtCtrl::default()
        };
        query_ext_ctrl(self.fd(), &mut query).ok()?;
        if query.flags & V4L2_CTRL_FLAG_DISABLED != 0 {
            return None;
        }
        Some(query.elem_size)
    }

    pub fn control_range(&self, id: u32) -> Option<(i64, i64, i64)> {
        let mut query = QueryExtCtrl {
            id,
            ..QueryExtCtrl::default()
        };
        query_ext_ctrl(self.fd(), &mut query).ok()?;
        if query.flags & V4L2_CTRL_FLAG_DISABLED != 0 {
            return None;
        }
        Some((query.minimum, query.maximum, query.default_value))
    }

    /// Which spelling of the HEVC controls the driver offers; the sizes must agree.
    pub fn hevc_naming(&self) -> Result<Naming, Error> {
        for naming in [Naming::Stateless, Naming::MpegVideo] {
            let Some(size) = self.control_size(Kind::HevcSliceParams.id_for(naming)) else {
                continue;
            };
            if size as usize == Kind::HevcSliceParams.size() {
                return Ok(naming);
            }
            return Err(Error::Unavailable(format!(
                "{} の HEVC_SLICE_PARAMS は {size} バイトで、このビルドは {} バイトを書きます\
                 （Linux 6.0 より前の hevc-ctrls.h は構造体そのものが違います）",
                self.driver,
                Kind::HevcSliceParams.size(),
            )));
        }
        Err(Error::Unavailable(format!(
            "{} に HEVC の stateless 制御がありません",
            self.driver
        )))
    }

    pub fn check_controls(&self, naming: Naming, wanted: &[Kind]) -> Result<(), Error> {
        for kind in wanted {
            let id = kind.id_for(naming);
            let Some(size) = self.control_size(id) else {
                return Err(Error::Unavailable(format!(
                    "{} に {} がありません",
                    self.driver,
                    kind.name()
                )));
            };
            if size as usize != kind.size() {
                return Err(Error::Unavailable(format!(
                    "{} の {} は {size} バイトで、このビルドは {} バイトを書きます",
                    self.driver,
                    kind.name(),
                    kind.size()
                )));
            }
        }
        Ok(())
    }

    pub fn set_controls(
        &self,
        naming: Naming,
        controls: &mut [Ctl],
        request: Option<RawFd>,
    ) -> Result<(), Error> {
        if controls.is_empty() {
            return Ok(());
        }
        let mut entries: Vec<ExtControl> = Vec::with_capacity(controls.len());
        for control in controls.iter_mut() {
            let id = control.kind.id_for(naming);
            entries.push(match control.value {
                Some(value) => ExtControl {
                    id,
                    size: 0,
                    reserved2: [0],
                    value: ExtControlValue { value },
                },
                None => ExtControl {
                    id,
                    size: control.bytes.len() as u32,
                    reserved2: [0],
                    value: ExtControlValue {
                        ptr: control.bytes.as_mut_ptr().cast::<c_void>(),
                    },
                },
            });
        }
        let mut set = ExtControls {
            which: match request {
                Some(_) => V4L2_CTRL_WHICH_REQUEST_VAL,
                None => V4L2_CTRL_WHICH_CUR_VAL,
            },
            count: entries.len() as u32,
            error_idx: 0,
            request_fd: request.unwrap_or(0),
            reserved: [0],
            controls: entries.as_mut_ptr(),
        };
        // SAFETY: `count` live entries, each `ptr` at `size` live bytes borrowed for the call.
        unsafe { ioctl(self.fd(), VIDIOC_S_EXT_CTRLS, &mut set) }.map_err(|error| {
            let at = set.error_idx as usize;
            let name = controls.get(at).map_or("?", |control| control.kind.name());
            failed(&format!("VIDIOC_S_EXT_CTRLS({name})"), error)
        })
    }

    pub fn decoder_stop(&self) -> Result<(), Error> {
        let mut command = DecoderCmd {
            cmd: V4L2_DEC_CMD_STOP,
            ..DecoderCmd::default()
        };
        decoder_cmd(self.fd(), &mut command).map_err(|error| failed("VIDIOC_DECODER_CMD", error))
    }

    pub fn subscribe(&self, kind: u32) -> Result<(), Error> {
        let mut subscription = EventSubscription {
            kind,
            ..EventSubscription::default()
        };
        subscribe_event(self.fd(), &mut subscription)
            .map_err(|error| failed("VIDIOC_SUBSCRIBE_EVENT", error))
    }

    pub fn event(&self) -> Option<(u32, u32)> {
        let mut event = Event::default();
        dqevent(self.fd(), &mut event).ok()?;
        let changes = u32::from_ne_bytes([event.u[0], event.u[1], event.u[2], event.u[3]]);
        Some((event.kind, changes))
    }

    pub fn control_value(&self, id: u32) -> Option<i32> {
        let mut control = Control { id, value: 0 };
        g_ctrl(self.fd(), &mut control).ok()?;
        Some(control.value)
    }

    /// Where inside the capture buffer the picture is; `None`: the whole buffer.
    pub fn compose(&self, kind: u32) -> Option<Rect> {
        let mut selection = Selection {
            kind: if kind == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE {
                V4L2_BUF_TYPE_VIDEO_CAPTURE
            } else {
                V4L2_BUF_TYPE_VIDEO_OUTPUT
            },
            target: V4L2_SEL_TGT_COMPOSE,
            ..Selection::default()
        };
        g_selection(self.fd(), &mut selection).ok()?;
        (selection.r.width > 0 && selection.r.height > 0).then_some(selection.r)
    }

    /// Without the picture size: told the size first, some drivers send no source change.
    pub fn set_compressed(
        &self,
        kind: u32,
        pixelformat: u32,
        sizeimage: u32,
    ) -> Result<PixFormatMplane, Error> {
        let mut format = self.get_format(kind)?;
        format.kind = kind;
        format.pix_mp.pixelformat = pixelformat;
        format.pix_mp.num_planes = 1;
        format.pix_mp.plane_fmt[0].sizeimage = sizeimage;
        self.put_format(format)
    }

    pub fn set_raw_format(
        &self,
        kind: u32,
        pixelformat: u32,
        width: u32,
        height: u32,
        field: u32,
    ) -> Result<PixFormatMplane, Error> {
        let mut format = self.get_format(kind)?;
        format.kind = kind;
        format.pix_mp.pixelformat = pixelformat;
        format.pix_mp.width = width;
        format.pix_mp.height = height;
        format.pix_mp.field = field;
        format.pix_mp.num_planes = 1;
        format.pix_mp.plane_fmt[0].bytesperline = 0;
        format.pix_mp.plane_fmt[0].sizeimage = 0;
        self.put_format(format)
    }

    /// Not [`Device::set_format`]: its `V4L2_FIELD_NONE` would ask for progressive.
    pub fn set_pixelformat(&self, kind: u32, pixelformat: u32) -> Result<PixFormatMplane, Error> {
        let mut format = self.get_format(kind)?;
        if format.pix_mp.pixelformat == pixelformat {
            return Ok(format.pix_mp);
        }
        format.kind = kind;
        format.pix_mp.pixelformat = pixelformat;
        self.put_format(format)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Enqueue {
    pub kind: u32,
    pub index: u32,
    pub bytes: u32,
    pub ts: u64,
    pub request: Option<RawFd>,
    /// この出力バッファはフレームの最後ではない。
    pub hold: bool,
    /// QBUF on a multi-planar queue rejects a short count.
    pub planes_used: u32,
    /// On a deinterlacer's output queue an instruction; without it nothing is woven.
    pub field: u32,
}

impl Enqueue {
    pub fn of(kind: u32, index: u32) -> Self {
        Self {
            kind,
            index,
            bytes: 0,
            ts: 0,
            request: None,
            hold: false,
            planes_used: 1,
            field: V4L2_FIELD_NONE,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Poll {
    pub capture: bool,
    pub output: bool,
    pub event: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

const POLLIN: i16 = 0x001;
const POLLOUT: i16 = 0x004;
const POLLPRI: i16 = 0x002;
const O_NONBLOCK: i32 = 0o4000;
const EAGAIN: c_int = 11;
const EPIPE: c_int = 32;

unsafe extern "C" {
    #[link_name = "poll"]
    fn poll_raw(fds: *mut PollFd, count: usize, timeout: c_int) -> c_int;
}

#[derive(Debug, Clone, Copy)]
pub struct Dequeued {
    pub index: usize,
    pub ts: u64,
    pub error: bool,
    pub last: bool,
    pub bytes: u32,
    pub field: u32,
}

#[derive(Debug)]
pub struct Request(OwnedFd);

impl Request {
    pub fn alloc(media: &File) -> Result<Self, Error> {
        let mut fd: i32 = -1;
        request_alloc(media.as_raw_fd(), &mut fd)
            .map_err(|error| failed("MEDIA_IOC_REQUEST_ALLOC", error))?;
        if fd < 0 {
            return Err(Error::Unavailable(String::from(
                "MEDIA_IOC_REQUEST_ALLOC gave no descriptor",
            )));
        }
        // SAFETY: the kernel has just made this descriptor for this process.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    pub fn fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }

    pub fn queue(&self) -> Result<(), Error> {
        ioctl_bare(self.fd(), Bare::RequestQueue)
            .map_err(|error| failed("MEDIA_REQUEST_IOC_QUEUE", error))
    }

    pub fn reinit(&self) -> Result<(), Error> {
        ioctl_bare(self.fd(), Bare::RequestReinit)
            .map_err(|error| failed("MEDIA_REQUEST_IOC_REINIT", error))
    }
}

/// Found by the topology's device number; guessing by index breaks with two decoders.
pub fn media_for(video: &Device) -> Result<File, Error> {
    let want = device_number(&video.path)?;
    let mut tried = 0;
    for entry in std::fs::read_dir("/dev").into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("media") {
            continue;
        }
        let Ok(file) = File::options().read(true).write(true).open(&path) else {
            continue;
        };
        tried += 1;
        if topology_has(&file, want) {
            return Ok(file);
        }
    }
    Err(Error::Unavailable(format!(
        "{} を持つ media デバイスがありません（{tried} 個試しました）",
        video.path
    )))
}

fn device_number(path: &str) -> Result<(u32, u32), Error> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|error| failed(path, error))?;
    let rdev = meta.rdev();
    // glibc's `makedev` encoding, which scatters the bits.
    let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfffu64);
    let minor = (rdev & 0xff) | ((rdev >> 12) & !0xffu64);
    Ok((major as u32, minor as u32))
}

fn topology_has(media: &File, want: (u32, u32)) -> bool {
    let mut topology = MediaV2Topology::default();
    // SAFETY: with every pointer null only the counts are written.
    if unsafe { ioctl(media.as_raw_fd(), MEDIA_IOC_G_TOPOLOGY, &mut topology) }.is_err() {
        return false;
    }
    let count = topology.num_interfaces as usize;
    if count == 0 || count > 4096 {
        return false;
    }
    let mut interfaces = vec![MediaV2Interface::default(); count];
    let mut ask = MediaV2Topology {
        num_interfaces: count as u32,
        ptr_interfaces: interfaces.as_mut_ptr() as u64,
        ..MediaV2Topology::default()
    };
    // SAFETY: `ptr_interfaces` has `num_interfaces` live entries; other pointers are null.
    if unsafe { ioctl(media.as_raw_fd(), MEDIA_IOC_G_TOPOLOGY, &mut ask) }.is_err() {
        return false;
    }
    interfaces.iter().any(|interface| {
        interface.intf_type == MEDIA_INTF_T_V4L_VIDEO
            && (interface.devnode.major, interface.devnode.minor) == want
    })
}

fn text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_number_is_taken_apart_the_way_the_library_puts_it_together() {
        let rdev: u64 = ((81u64 & 0xfff) << 8) | (3u64 & 0xff);
        let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfffu64);
        let minor = (rdev & 0xff) | ((rdev >> 12) & !0xffu64);
        assert_eq!((major, minor), (81, 3));
    }

    #[test]
    fn a_name_stops_at_the_first_nought() {
        assert_eq!(text(b"rpivid\0\0\0\0\0\0\0\0\0\0"), "rpivid");
        assert_eq!(text(b"hantro-vpu"), "hantro-vpu");
    }
}
