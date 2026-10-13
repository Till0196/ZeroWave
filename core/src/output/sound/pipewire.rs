// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! PipeWire, in front of ALSA on a desktop. An `iec958` stream with `MPEG2-AAC` opens the ALSA
//! device with the non-audio bit set. A sink offers the codec only if the EDID said so.
//! The format POD is written by hand: `spa_pod_builder_*` are `static inline`.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use crate::dl::Object;

macro_rules! symbol {
    ($object:expr, $name:literal) => {
        // SAFETY: the name is an entry point of the library just opened, bound to its signature.
        unsafe { $object.symbol($name) }.map_err(|missing| Error::Unavailable(missing.0))?
    };
}

use super::pump::{FRAME, Pump};
use super::{Error, Link};

// The type numbers are the ABI.
const TYPE_ID: u32 = 3;
const TYPE_INT: u32 = 4;
const TYPE_OBJECT: u32 = 15;
const OBJECT_FORMAT: u32 = 0x0004_0003;
const PARAM_ENUM_FORMAT: u32 = 3;
const FORMAT_MEDIA_TYPE: u32 = 1;
const FORMAT_MEDIA_SUBTYPE: u32 = 2;
const FORMAT_AUDIO_RATE: u32 = 0x0001_0003;
const FORMAT_AUDIO_IEC958_CODEC: u32 = 0x0001_0006;
const MEDIA_TYPE_AUDIO: u32 = 1;
const MEDIA_SUBTYPE_IEC958: u32 = 3;
const CODEC_MPEG2_AAC: u32 = 5;

const DIRECTION_OUTPUT: c_int = 1;
/// The target is named in a property (by-id is deprecated).
const ID_ANY: u32 = u32::MAX;
/// Not `RT_PROCESS`: the callback takes a mutex.
const FLAG_AUTOCONNECT_MAP: u32 = (1 << 0) | (1 << 2);
const STATE_ERROR: c_int = -1;
const STATE_STREAMING: c_int = 3;
const VERSION_STREAM_EVENTS: u32 = 2;

const PATIENCE: Duration = Duration::from_secs(2);

const POD_BYTES: usize = 8 + 8 + 4 * 24;

#[repr(C, align(8))]
#[derive(Debug)]
struct Pod([u8; POD_BYTES]);

fn format_pod(rate: u32) -> Pod {
    fn word(out: &mut [u8], at: &mut usize, value: u32) {
        out[*at..*at + 4].copy_from_slice(&value.to_ne_bytes());
        *at += 4;
    }
    fn property(out: &mut [u8], at: &mut usize, key: u32, kind: u32, value: u32) {
        word(out, at, key);
        word(out, at, 0);
        word(out, at, 4);
        word(out, at, kind);
        word(out, at, value);
        word(out, at, 0);
    }

    let mut bytes = [0u8; POD_BYTES];
    let mut at = 0;
    word(&mut bytes, &mut at, (POD_BYTES - 8) as u32);
    word(&mut bytes, &mut at, TYPE_OBJECT);
    word(&mut bytes, &mut at, OBJECT_FORMAT);
    word(&mut bytes, &mut at, PARAM_ENUM_FORMAT);
    property(
        &mut bytes,
        &mut at,
        FORMAT_MEDIA_TYPE,
        TYPE_ID,
        MEDIA_TYPE_AUDIO,
    );
    property(
        &mut bytes,
        &mut at,
        FORMAT_MEDIA_SUBTYPE,
        TYPE_ID,
        MEDIA_SUBTYPE_IEC958,
    );
    property(
        &mut bytes,
        &mut at,
        FORMAT_AUDIO_IEC958_CODEC,
        TYPE_ID,
        CODEC_MPEG2_AAC,
    );
    property(&mut bytes, &mut at, FORMAT_AUDIO_RATE, TYPE_INT, rate);
    debug_assert_eq!(at, POD_BYTES);
    Pod(bytes)
}

#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
    user_data: *mut c_void,
    size: u64,
    requested: u64,
    time: u64,
}

#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

#[repr(C)]
struct SpaData {
    kind: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

/// Whole because its size is passed to `pw_stream_get_time_n`.
#[repr(C)]
#[derive(Default)]
struct PwTime {
    now: i64,
    rate_num: u32,
    rate_denom: u32,
    ticks: u64,
    delay: i64,
    queued: u64,
    buffered: u64,
    queued_buffers: u32,
    avail_buffers: u32,
    size: u64,
}

/// Every slot present because the library indexes into it.
#[repr(C)]
struct Events {
    version: u32,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Option<unsafe extern "C" fn(*mut c_void)>,
    command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
}

type Handle = *mut c_void;

struct Symbols {
    init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char),
    properties_new: unsafe extern "C" fn(*const c_char, ...) -> Handle,
    properties_set: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    thread_loop_new: unsafe extern "C" fn(*const c_char, Handle) -> Handle,
    thread_loop_destroy: unsafe extern "C" fn(Handle),
    thread_loop_start: unsafe extern "C" fn(Handle) -> c_int,
    thread_loop_stop: unsafe extern "C" fn(Handle),
    thread_loop_lock: unsafe extern "C" fn(Handle),
    thread_loop_unlock: unsafe extern "C" fn(Handle),
    thread_loop_get_loop: unsafe extern "C" fn(Handle) -> Handle,
    stream_new_simple:
        unsafe extern "C" fn(Handle, *const c_char, Handle, *const Events, *mut c_void) -> Handle,
    stream_destroy: unsafe extern "C" fn(Handle),
    stream_connect:
        unsafe extern "C" fn(Handle, c_int, u32, u32, *const *const c_void, u32) -> c_int,
    stream_get_state: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    dequeue_buffer: unsafe extern "C" fn(Handle) -> *mut PwBuffer,
    queue_buffer: unsafe extern "C" fn(Handle, *mut PwBuffer) -> c_int,
    get_time_n: unsafe extern "C" fn(Handle, *mut PwTime, usize) -> c_int,
}

impl Symbols {
    fn load(object: &Object) -> Result<Self, Error> {
        Ok(Self {
            init: symbol!(object, b"pw_init\0"),
            properties_new: symbol!(object, b"pw_properties_new\0"),
            properties_set: symbol!(object, b"pw_properties_set\0"),
            thread_loop_new: symbol!(object, b"pw_thread_loop_new\0"),
            thread_loop_destroy: symbol!(object, b"pw_thread_loop_destroy\0"),
            thread_loop_start: symbol!(object, b"pw_thread_loop_start\0"),
            thread_loop_stop: symbol!(object, b"pw_thread_loop_stop\0"),
            thread_loop_lock: symbol!(object, b"pw_thread_loop_lock\0"),
            thread_loop_unlock: symbol!(object, b"pw_thread_loop_unlock\0"),
            thread_loop_get_loop: symbol!(object, b"pw_thread_loop_get_loop\0"),
            stream_new_simple: symbol!(object, b"pw_stream_new_simple\0"),
            stream_destroy: symbol!(object, b"pw_stream_destroy\0"),
            stream_connect: symbol!(object, b"pw_stream_connect\0"),
            stream_get_state: symbol!(object, b"pw_stream_get_state\0"),
            dequeue_buffer: symbol!(object, b"pw_stream_dequeue_buffer\0"),
            queue_buffer: symbol!(object, b"pw_stream_queue_buffer\0"),
            get_time_n: symbol!(object, b"pw_stream_get_time_n\0"),
        })
    }
}

struct Shared {
    pump: Arc<Pump>,
    symbols: Symbols,
    /// Atomic because the graph reads it from its thread.
    stream: AtomicPtr<c_void>,
    /// Frames of real content the graph still holds. `pw_time` also counts padding, so without
    /// this the clock stops a quantum short at the end of a recording.
    real: AtomicU64,
}

/// # Safety
///
/// `data` points into an `Arc<Shared>` that outlives the stream.
unsafe extern "C" fn process(data: *mut c_void) {
    if data.is_null() {
        return;
    }
    // SAFETY: the reference lives until `Drop` has destroyed the stream.
    let shared = unsafe { &*data.cast::<Shared>() };
    let stream = shared.stream.load(Ordering::Acquire);
    if stream.is_null() {
        return;
    }
    // SAFETY: the stream is the one this callback was registered on.
    let held = unsafe { (shared.symbols.dequeue_buffer)(stream) };
    if held.is_null() {
        return;
    }
    // SAFETY: a dequeued buffer has a `spa_buffer` with at least one data member.
    let (data, room) = unsafe {
        let buffer = &*(*held).buffer;
        if buffer.n_datas == 0 {
            (std::ptr::null_mut(), 0)
        } else {
            let first = &*buffer.datas;
            (first.data, first.maxsize as usize)
        }
    };
    if !data.is_null() {
        // SAFETY: `held` was just dequeued and is ours until it is queued.
        let asked = unsafe { (*held).requested } as usize * FRAME;
        let want = if asked == 0 { room } else { asked.min(room) };
        let mut bytes = Vec::new();
        shared.pump.try_take(want, &mut bytes);
        // SAFETY: `data` has `room` bytes and `bytes.len() <= want <= room`.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.cast::<u8>(), bytes.len());
            // The rest of the quantum is nought; on this link that is the receiver losing lock.
            std::ptr::write_bytes(data.cast::<u8>().add(bytes.len()), 0, want - bytes.len());
        }
        shared.pump.handed((bytes.len() / FRAME) as u64);
        let real = shared
            .real
            .load(Ordering::Relaxed)
            .saturating_sub((want / FRAME) as u64)
            + (bytes.len() / FRAME) as u64;
        shared.real.store(real, Ordering::Relaxed);

        // SAFETY: the chunk belongs to the data member just written.
        unsafe {
            let first = &*(*(*held).buffer).datas;
            if !first.chunk.is_null() {
                (*first.chunk).offset = 0;
                (*first.chunk).size = want as u32;
                (*first.chunk).stride = FRAME as i32;
            }
            (*held).size = (want / FRAME) as u64;
        }
    }
    // The clock follows the wire: the graph's constant delay plus the real content.
    let mut time = PwTime::default();
    // SAFETY: `time` is a valid place of the size given.
    let delay =
        if unsafe { (shared.symbols.get_time_n)(stream, &mut time, size_of::<PwTime>()) } >= 0 {
            time.delay.max(0) as u64
        } else {
            0
        };
    shared
        .pump
        .holding(shared.real.load(Ordering::Relaxed) + delay);

    // SAFETY: the buffer was dequeued from this stream and is queued once.
    unsafe { (shared.symbols.queue_buffer)(stream, held) };
}

static STARTED: Once = Once::new();

pub struct PipeWire {
    shared: Arc<Shared>,
    loop_: Handle,
    events: Box<Events>,
    rate: u32,
    _object: Object,
}

impl std::fmt::Debug for PipeWire {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("PipeWire")
    }
}

// SAFETY: the handles are used from the player's thread only under `pw_thread_loop_lock`.
unsafe impl Send for PipeWire {}

impl PipeWire {
    /// `target` is a node name or object serial; `None` is the default sink.
    pub fn open(rate: u32, target: Option<&str>) -> Result<Self, Error> {
        let object = Object::open_any(&["libpipewire-0.3.so.0", "libpipewire-0.3.so"])
            .map_err(|missing| Error::Unavailable(missing.0))?;
        let symbols = Symbols::load(&object)?;

        // SAFETY: no arguments, as a library embedding PipeWire passes, and only once.
        STARTED.call_once(|| unsafe {
            (symbols.init)(std::ptr::null_mut(), std::ptr::null_mut());
        });

        let name = CString::new("zerowave").expect("no nought in it");
        // SAFETY: a nought-terminated name; a null user loop asks for one of its own.
        let loop_ = unsafe { (symbols.thread_loop_new)(name.as_ptr(), std::ptr::null_mut()) };
        if loop_.is_null() {
            return Err(Error::Unavailable(String::from(
                "PipeWire のループが作れない",
            )));
        }

        let shared = Arc::new(Shared {
            pump: Arc::new(Pump::new()),
            symbols,
            stream: AtomicPtr::new(std::ptr::null_mut()),
            real: AtomicU64::new(0),
        });
        let events = Box::new(Events {
            version: VERSION_STREAM_EVENTS,
            destroy: None,
            state_changed: None,
            control_info: None,
            io_changed: None,
            param_changed: None,
            add_buffer: None,
            remove_buffer: None,
            process: Some(process),
            drained: None,
            command: None,
            trigger_done: None,
        });

        let link = Self {
            shared,
            loop_,
            events,
            rate,
            _object: object,
        };
        link.start(target)?;
        Ok(link)
    }

    /// Connects under `pw_thread_loop_lock` as PipeWire's examples do.
    fn start(&self, target: Option<&str>) -> Result<(), Error> {
        let symbols = &self.shared.symbols;
        self.make(target)?;
        // SAFETY: the loop was made here and is started once.
        if unsafe { (symbols.thread_loop_start)(self.loop_) } < 0 {
            return Err(Error::Unavailable(String::from(
                "PipeWire のループが始まらない",
            )));
        }
        // SAFETY: the loop is running and is not locked by this thread.
        unsafe { (symbols.thread_loop_lock)(self.loop_) };
        let result = self.connect();
        // SAFETY: locked just above by this thread.
        unsafe { (symbols.thread_loop_unlock)(self.loop_) };
        result?;
        self.settle()
    }

    fn make(&self, target: Option<&str>) -> Result<(), Error> {
        let symbols = &self.shared.symbols;
        let props = self.properties(target)?;
        let name = CString::new("ZeroWave").expect("no nought in it");
        // SAFETY: the name outlives the call, `props` is taken, and `self` owns `events`.
        let stream = unsafe {
            (symbols.stream_new_simple)(
                (symbols.thread_loop_get_loop)(self.loop_),
                name.as_ptr(),
                props,
                &*self.events,
                Arc::as_ptr(&self.shared) as *mut c_void,
            )
        };
        if stream.is_null() {
            return Err(Error::Unavailable(String::from(
                "PipeWire のストリームが作れない",
            )));
        }
        self.shared.stream.store(stream, Ordering::Release);
        Ok(())
    }

    fn connect(&self) -> Result<(), Error> {
        let symbols = &self.shared.symbols;
        let stream = self.shared.stream.load(Ordering::Acquire);
        let pod = format_pod(self.rate);
        let params = [pod.0.as_ptr().cast::<c_void>()];
        // SAFETY: the stream was made above; `pw_stream_connect` copies the POD it keeps.
        let status = unsafe {
            (symbols.stream_connect)(
                stream,
                DIRECTION_OUTPUT,
                ID_ANY,
                FLAG_AUTOCONNECT_MAP,
                params.as_ptr(),
                1,
            )
        };
        if status < 0 {
            return Err(Error::Unavailable(format!(
                "PipeWire が繋げない ({status})"
            )));
        }
        Ok(())
    }

    fn properties(&self, target: Option<&str>) -> Result<Handle, Error> {
        let symbols = &self.shared.symbols;
        let pairs = [
            (c"media.type", c"Audio"),
            (c"media.category", c"Playback"),
            (c"media.role", c"Movie"),
            (c"node.name", c"zerowave"),
        ];
        // SAFETY: a nought-terminated list of nought-terminated pairs, as the variadic asks.
        let props = unsafe {
            (symbols.properties_new)(
                pairs[0].0.as_ptr(),
                pairs[0].1.as_ptr(),
                pairs[1].0.as_ptr(),
                pairs[1].1.as_ptr(),
                pairs[2].0.as_ptr(),
                pairs[2].1.as_ptr(),
                pairs[3].0.as_ptr(),
                pairs[3].1.as_ptr(),
                std::ptr::null::<c_char>(),
            )
        };
        if props.is_null() {
            return Err(Error::Unavailable(String::from(
                "PipeWire の properties が作れない",
            )));
        }
        if let Some(target) = target {
            let name =
                CString::new(target).map_err(|_| Error::Unsupported("a target with a nought"))?;
            // SAFETY: `props` was just made and the call copies both strings.
            unsafe { (symbols.properties_set)(props, c"target.object".as_ptr(), name.as_ptr()) };
        }
        Ok(props)
    }

    /// A stream that never leaves `CONNECTING` is a receiver that does not decode AAC.
    fn settle(&self) -> Result<(), Error> {
        let symbols = &self.shared.symbols;
        let stream = self.shared.stream.load(Ordering::Acquire);
        let until = Instant::now() + PATIENCE;
        loop {
            let mut what: *const c_char = std::ptr::null();
            // SAFETY: the stream is connected, `what` is a valid place, and the loop is locked.
            let (state, why) = unsafe {
                (symbols.thread_loop_lock)(self.loop_);
                let state = (symbols.stream_get_state)(stream, &mut what);
                let why =
                    (!what.is_null()).then(|| CStr::from_ptr(what).to_string_lossy().into_owned());
                (symbols.thread_loop_unlock)(self.loop_);
                (state, why)
            };
            if state == STATE_STREAMING {
                return Ok(());
            }
            if state == STATE_ERROR {
                return Err(Error::Unavailable(why.unwrap_or_else(|| {
                    String::from("PipeWire がフォーマットを拒否した")
                })));
            }
            if Instant::now() >= until {
                return Err(Error::Unavailable(String::from(
                    "PipeWire が MPEG2-AAC に対応するポートを見つけられない",
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Link for PipeWire {
    fn write(&mut self, burst: &[u8]) -> Result<usize, Error> {
        self.shared.pump.push(burst);
        Ok(burst.len() / FRAME)
    }

    fn played(&self) -> u64 {
        self.shared.pump.played()
    }

    fn queued(&self) -> usize {
        self.shared.pump.queued()
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn name(&self) -> &'static str {
        "PipeWire"
    }
}

impl Drop for PipeWire {
    fn drop(&mut self) {
        let symbols = &self.shared.symbols;
        self.shared.pump.close();
        let stream = self
            .shared
            .stream
            .swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: the stream is destroyed under the lock (stopping the callback), then the loop.
        unsafe {
            (symbols.thread_loop_lock)(self.loop_);
            if !stream.is_null() {
                (symbols.stream_destroy)(stream);
            }
            (symbols.thread_loop_unlock)(self.loop_);
            (symbols.thread_loop_stop)(self.loop_);
            (symbols.thread_loop_destroy)(self.loop_);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{POD_BYTES, format_pod};

    #[test]
    fn the_format_pod_is_the_shape_the_headers_say() {
        let pod = format_pod(48000);
        let word = |at: usize| u32::from_ne_bytes(pod.0[at..at + 4].try_into().expect("four"));

        assert_eq!(pod.0.len(), 112);
        assert_eq!(word(0) as usize, POD_BYTES - 8);
        assert_eq!(word(4), super::TYPE_OBJECT);
        assert_eq!(word(8), super::OBJECT_FORMAT);
        assert_eq!(word(12), super::PARAM_ENUM_FORMAT);

        let expected = [
            (
                super::FORMAT_MEDIA_TYPE,
                super::TYPE_ID,
                super::MEDIA_TYPE_AUDIO,
            ),
            (
                super::FORMAT_MEDIA_SUBTYPE,
                super::TYPE_ID,
                super::MEDIA_SUBTYPE_IEC958,
            ),
            (
                super::FORMAT_AUDIO_IEC958_CODEC,
                super::TYPE_ID,
                super::CODEC_MPEG2_AAC,
            ),
            (super::FORMAT_AUDIO_RATE, super::TYPE_INT, 48000),
        ];
        for (which, (key, kind, value)) in expected.into_iter().enumerate() {
            let at = 16 + which * 24;
            assert_eq!(word(at), key, "key of property {which}");
            assert_eq!(word(at + 4), 0, "flags of property {which}");
            assert_eq!(word(at + 8), 4, "value size of property {which}");
            assert_eq!(word(at + 12), kind, "value type of property {which}");
            assert_eq!(word(at + 16), value, "value of property {which}");
            assert_eq!(word(at + 20), 0, "padding of property {which}");
        }
    }

    #[test]
    fn the_pod_is_aligned_for_the_library() {
        let pod = format_pod(48000);
        assert_eq!(pod.0.as_ptr() as usize % 8, 0);
    }
}
