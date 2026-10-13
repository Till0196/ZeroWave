// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! `AudioTrack` with `ENCODING_IEC61937`. Not `ENCODING_AAC_LC`: TVs rarely list it and the
//! framework then silently decodes to stereo PCM.

use std::sync::Arc;
use std::thread::JoinHandle;

use super::jni::{Env, Method, Object, Value};
use super::pump::{FRAME, Pump};
use super::{Error, Link};

const ENCODING_IEC61937: i32 = 13;
/// Before Android S the framework requires exactly this.
const CHANNEL_OUT_STEREO: i32 = 12;
const STREAM_MUSIC: i32 = 3;
const MODE_STREAM: i32 = 1;
const STATE_INITIALIZED: i32 = 1;
const BUFFER: usize = 4 * 4096;

struct Track {
    env: Env,
    track: Object,
    array: Object,
    write: Method,
    head: Method,
    stop: Method,
    release: Method,
    frames: usize,
}

impl Track {
    /// # Safety
    ///
    /// `vm` has to be a `JavaVM *`.
    unsafe fn open(vm: *mut std::ffi::c_void, rate: u32) -> Result<Self, Error> {
        // SAFETY: by the contract above.
        let env = unsafe { Env::attach(vm) }?;
        env.sane()?;

        let class = env.class("android/media/AudioTrack")?;
        // クラスは途中で抜けても手放す。
        let made = (|| -> Result<_, Error> {
            let smallest = env.static_method(class, "getMinBufferSize", "(III)I")?;
            let least = env.call_static_int(
                class,
                smallest,
                &[
                    Value::int(rate as i32),
                    Value::int(CHANNEL_OUT_STEREO),
                    Value::int(ENCODING_IEC61937),
                ],
            )?;
            if least <= 0 {
                return Err(Error::Unavailable(String::from(
                    "この端末は IEC61937 のダイレクト出力トラックを持たない",
                )));
            }
            let bytes = BUFFER.max(least as usize).next_multiple_of(FRAME);

            // メソッドは作る前に取得する（作った後で失敗すると作った物が残る）。
            let state = env.method(class, "getState", "()I")?;
            let write = env.method(class, "write", "([SII)I")?;
            let head = env.method(class, "getPlaybackHeadPosition", "()I")?;
            let play = env.method(class, "play", "()V")?;
            let stop = env.method(class, "stop", "()V")?;
            let release = env.method(class, "release", "()V")?;
            // The deprecated constructor: one call instead of five.
            let make = env.method(class, "<init>", "(IIIIII)V")?;
            let track = env.new_object(
                class,
                make,
                &[
                    Value::int(STREAM_MUSIC),
                    Value::int(rate as i32),
                    Value::int(CHANNEL_OUT_STEREO),
                    Value::int(ENCODING_IEC61937),
                    Value::int(bytes as i32),
                    Value::int(MODE_STREAM),
                ],
            )?;
            Ok((bytes, state, write, head, play, stop, release, track))
        })();
        env.release(class);
        let (bytes, state, write, head, play, stop, release, track) = made?;

        // 作った後で失敗にするなら `release()` してから手放す（GC までリソースが残る）。
        let array = match (|| -> Result<_, Error> {
            if env.call_int(track, state, &[])? != STATE_INITIALIZED {
                return Err(Error::Unavailable(String::from(
                    "AudioTrack が初期化されない（IEC61937 に対応しない端末）",
                )));
            }
            let array = env.short_array(bytes / 2)?;
            if let Err(error) = env.call_void(track, play, &[]) {
                env.release(array);
                return Err(error);
            }
            Ok(array)
        })() {
            Ok(array) => array,
            Err(error) => {
                let _ = env.call_void(track, release, &[]);
                env.release(track);
                return Err(error);
            }
        };
        Ok(Self {
            env,
            track,
            array,
            write,
            head,
            stop,
            release,
            frames: bytes / FRAME,
        })
    }

    fn write(&self, bytes: &[u8], shorts: &mut Vec<i16>) -> Result<usize, Error> {
        shorts.clear();
        shorts.extend(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair)),
        );
        self.env.fill(self.array, shorts)?;
        let took = self.env.call_int(
            self.track,
            self.write,
            &[
                Value::object(self.array),
                Value::int(0),
                Value::int(shorts.len() as i32),
            ],
        )?;
        if took < 0 {
            return Err(Error::Lost(format!("AudioTrack.write が {took}")));
        }
        Ok(took as usize / 2)
    }

    /// Java's `int` wraps at about twelve hours, so the caller unwraps it.
    fn head(&self) -> Result<u32, Error> {
        Ok(self.env.call_int(self.track, self.head, &[])? as u32)
    }
}

impl Drop for Track {
    fn drop(&mut self) {
        let _ = self.env.call_void(self.track, self.stop, &[]);
        let _ = self.env.call_void(self.track, self.release, &[]);
        self.env.release(self.array);
        self.env.release(self.track);
    }
}

#[derive(Debug)]
pub struct AudioTrack {
    pump: Arc<Pump>,
    writer: Option<JoinHandle<()>>,
    rate: u32,
}

impl AudioTrack {
    /// The `JavaVM` comes from `ndk-context`; without one this is `Unavailable`.
    pub fn open(rate: u32) -> Result<Self, Error> {
        let vm = std::panic::catch_unwind(|| ndk_context::android_context().vm())
            .map_err(|_| Error::Unavailable(String::from("JavaVM がまだ渡されていない")))?;
        // SAFETY: `ndk_context` holds the process's own `JavaVM`.
        unsafe { Self::open_with(vm, rate) }
    }

    /// # Safety
    ///
    /// `vm` has to be a `JavaVM *` that outlives the link.
    pub unsafe fn open_with(vm: *mut std::ffi::c_void, rate: u32) -> Result<Self, Error> {
        let pump = Arc::new(Pump::new());
        let theirs = pump.clone();
        let held = Machine(vm);
        let (ready, heard) = std::sync::mpsc::channel();
        let writer = std::thread::Builder::new()
            .name(String::from("zerowave-audiotrack"))
            .spawn(move || {
                let held = held;
                // SAFETY: the machine outlives the link, which joins this thread first.
                let track = match unsafe { Track::open(held.0, rate) } {
                    Ok(track) => {
                        let _ = ready.send(Ok(()));
                        track
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                pour(&track, &theirs);
            })
            .map_err(|error| Error::Unavailable(error.to_string()))?;

        match heard.recv() {
            Ok(Ok(())) => Ok(Self {
                pump,
                writer: Some(writer),
                rate,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(Error::Unavailable(String::from("音声のスレッドが落ちた"))),
        }
    }
}

struct Machine(*mut std::ffi::c_void);

// SAFETY: a `JavaVM *` is usable from any thread.
unsafe impl Send for Machine {}

fn pour(track: &Track, pump: &Pump) {
    let want = track.frames * FRAME;
    let mut held = Vec::new();
    let mut shorts = Vec::new();
    let mut handed: u64 = 0;
    let mut turns: u64 = 0;
    let mut last: u32 = 0;
    while !pump.closing() {
        pump.take(want, &mut held);
        if held.is_empty() {
            // Ask even when idle, or the clock never reaches the end of a recording.
            if let Ok(head) = track.head() {
                if head < last {
                    turns += 1;
                }
                last = head;
                pump.holding(handed.saturating_sub(turns * (1 << 32) + u64::from(head)));
            }
            continue;
        }
        match track.write(&held, &mut shorts) {
            Ok(frames) => {
                handed += frames as u64;
                pump.handed(frames as u64);
            }
            Err(error) => {
                tracing::warn!("音声（AudioTrack）: {error}");
                pump.die();
                return;
            }
        }
        match track.head() {
            Ok(head) => {
                if head < last {
                    turns += 1;
                }
                last = head;
                let played = turns * (1 << 32) + u64::from(head);
                pump.holding(handed.saturating_sub(played));
            }
            Err(error) => {
                tracing::warn!("音声（AudioTrack）: {error}");
                pump.die();
                return;
            }
        }
    }
}

impl Link for AudioTrack {
    fn write(&mut self, burst: &[u8]) -> Result<usize, Error> {
        if !self.pump.push(burst) {
            return Err(Error::Unavailable(String::from("音声の出力が止まった")));
        }
        Ok(burst.len() / FRAME)
    }

    fn played(&self) -> u64 {
        self.pump.played()
    }

    fn queued(&self) -> usize {
        self.pump.queued()
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn name(&self) -> &'static str {
        "AudioTrack"
    }
}

impl Drop for AudioTrack {
    fn drop(&mut self) {
        self.pump.close();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}
