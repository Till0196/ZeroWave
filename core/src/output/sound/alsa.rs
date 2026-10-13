// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ALSA, opened directly, for a machine with no sound server.
//! `AES0=0x6` in the device name is the non-audio bit; without it the burst plays as loud noise.

use std::ffi::{CString, c_char, c_int, c_uint, c_void};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::dl::Object;

macro_rules! symbol {
    ($object:expr, $name:literal) => {
        // SAFETY: the name is an entry point of the library just opened, bound to its signature.
        unsafe { $object.symbol($name) }.map_err(|missing| Error::Unavailable(missing.0))?
    };
}

use super::pump::{FRAME, Pump};
use super::{Error, Link};

const PLAYBACK: c_int = 0;
const S16_LE: c_int = 2;
const INTERLEAVED: c_int = 3;
/// About ten bursts.
const LATENCY: c_uint = 200_000;

const NAMES: [&str; 2] = [
    "iec958:AES0=0x6,AES1=0x82,AES2=0x0,AES3=0x2",
    "hdmi:AES0=0x6,AES1=0x82,AES2=0x0,AES3=0x2",
];

type Pcm = *mut c_void;

struct Symbols {
    open: unsafe extern "C" fn(*mut Pcm, *const c_char, c_int, c_int) -> c_int,
    close: unsafe extern "C" fn(Pcm) -> c_int,
    set_params: unsafe extern "C" fn(Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    writei: unsafe extern "C" fn(Pcm, *const c_void, c_ulong) -> c_long,
    delay: unsafe extern "C" fn(Pcm, *mut c_long) -> c_int,
    recover: unsafe extern "C" fn(Pcm, c_int, c_int) -> c_int,
    drain: unsafe extern "C" fn(Pcm) -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
}

#[allow(non_camel_case_types)]
type c_long = isize;
#[allow(non_camel_case_types)]
type c_ulong = usize;

struct Device {
    symbols: Symbols,
    pcm: Pcm,
    _object: Object,
}

// SAFETY: `snd_pcm_t` はスレッドに結びつかず、渡った先の書くスレッドだけが使う。
unsafe impl Send for Device {}

impl Device {
    fn open(rate: u32, on: Option<&str>) -> Result<Self, Error> {
        let object = Object::open_any(&["libasound.so.2", "libasound.so"])
            .map_err(|missing| Error::Unavailable(missing.0))?;
        let symbols = Symbols {
            open: symbol!(object, b"snd_pcm_open\0"),
            close: symbol!(object, b"snd_pcm_close\0"),
            set_params: symbol!(object, b"snd_pcm_set_params\0"),
            writei: symbol!(object, b"snd_pcm_writei\0"),
            delay: symbol!(object, b"snd_pcm_delay\0"),
            recover: symbol!(object, b"snd_pcm_recover\0"),
            drain: symbol!(object, b"snd_pcm_drain\0"),
            strerror: symbol!(object, b"snd_strerror\0"),
        };

        let mut last = Error::Unavailable(String::from("IEC958 の出力が無い"));
        let named = [on.unwrap_or_default()];
        let names: &[&str] = if on.is_some() { &named } else { &NAMES };
        for name in names.iter().copied() {
            match Self::start(&symbols, name, rate) {
                Ok(pcm) => {
                    return Ok(Self {
                        symbols,
                        pcm,
                        _object: object,
                    });
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn start(symbols: &Symbols, name: &str, rate: u32) -> Result<Pcm, Error> {
        let text = CString::new(name).map_err(|_| Error::Unsupported("a name with a nought"))?;
        let mut pcm: Pcm = std::ptr::null_mut();
        // SAFETY: valid out-pointer; `text` outlives the call.
        let status = unsafe { (symbols.open)(&mut pcm, text.as_ptr(), PLAYBACK, 0) };
        if status < 0 {
            return Err(Error::Unavailable(format!(
                "{name}: {}",
                strerror(symbols, status)
            )));
        }
        // SAFETY: `pcm` is open. No resampling: a rate it cannot take must be refused.
        let status = unsafe { (symbols.set_params)(pcm, S16_LE, INTERLEAVED, 2, rate, 0, LATENCY) };
        if status < 0 {
            // SAFETY: open, not used again.
            unsafe { (symbols.close)(pcm) };
            return Err(Error::Unavailable(format!(
                "{name}: {}",
                strerror(symbols, status)
            )));
        }
        Ok(pcm)
    }

    fn write(&self, bytes: &[u8]) -> Result<usize, Error> {
        let mut at = 0;
        while at < bytes.len() {
            let frames = (bytes.len() - at) / FRAME;
            // SAFETY: `bytes[at..]` holds `frames` whole frames.
            let took = unsafe {
                (self.symbols.writei)(self.pcm, bytes[at..].as_ptr().cast(), frames as c_ulong)
            };
            if took < 0 {
                // SAFETY: the handle is open.
                let status = unsafe { (self.symbols.recover)(self.pcm, took as c_int, 1) };
                if status < 0 {
                    return Err(Error::Lost(strerror(&self.symbols, status)));
                }
                continue;
            }
            at += took as usize * FRAME;
        }
        Ok(at / FRAME)
    }

    fn holding(&self) -> u64 {
        let mut frames: c_long = 0;
        // SAFETY: the handle is open.
        let status = unsafe { (self.symbols.delay)(self.pcm, &mut frames) };
        if status < 0 || frames < 0 {
            return 0;
        }
        frames as u64
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: drained once and closed once.
        unsafe {
            (self.symbols.drain)(self.pcm);
            (self.symbols.close)(self.pcm);
        }
    }
}

fn strerror(symbols: &Symbols, status: c_int) -> String {
    // SAFETY: returns a static string for any value.
    let text = unsafe { (symbols.strerror)(status) };
    if text.is_null() {
        return format!("ALSA error {status}");
    }
    // SAFETY: non-null static C string.
    unsafe { std::ffi::CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug)]
pub struct Alsa {
    pump: Arc<Pump>,
    writer: Option<JoinHandle<()>>,
    rate: u32,
}

impl Alsa {
    /// `on` is used as it is: a name without `AES0=0x6` plays the burst as samples.
    pub fn open(rate: u32, on: Option<&str>) -> Result<Self, Error> {
        let device = Device::open(rate, on)?;
        let pump = Arc::new(Pump::new());
        let theirs = pump.clone();
        let writer = std::thread::Builder::new()
            .name(String::from("zerowave-alsa"))
            .spawn(move || pour(&device, &theirs))
            .map_err(|error| Error::Unavailable(error.to_string()))?;
        Ok(Self {
            pump,
            writer: Some(writer),
            rate,
        })
    }
}

fn pour(device: &Device, pump: &Pump) {
    let mut held = Vec::new();
    while !pump.closing() {
        pump.take(64 * 1024, &mut held);
        // Ask the delay even when idle, or the clock never reaches the end of a recording.
        if held.is_empty() {
            pump.holding(device.holding());
            continue;
        }
        match device.write(&held) {
            Ok(frames) => {
                pump.handed(frames as u64);
                pump.holding(device.holding());
            }
            Err(error) => {
                tracing::warn!("音声（ALSA）: {error}");
                pump.die();
                return;
            }
        }
    }
}

impl Link for Alsa {
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
        "ALSA"
    }
}

impl Drop for Alsa {
    fn drop(&mut self) {
        self.pump.close();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}
