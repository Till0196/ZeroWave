// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Linux の音声出力。libasound（alsa-lib）を実行時に開き（[`zwcore::dl::Object`]）、書き込みのスレッドが
//! キュー（[`Pcm`]）から取り出して `snd_pcm_writei` で渡す。ビルドに libasound は要らず、無い機械では音声だけ出ない。
//! PulseAudio・PipeWire の機械でも、既定の `default` は ALSA のプラグインでサーバーへ行く。

#[cfg(feature = "sound")]
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
#[cfg(feature = "sound")]
use std::sync::Arc;
#[cfg(feature = "sound")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "sound")]
use zwcore::dl::Object;
#[cfg(feature = "sound")]
use zwcore::sound::Error;

#[cfg(feature = "sound")]
use super::queue::Parts;
#[cfg(feature = "sound")]
pub use super::queue::Pcm;

#[cfg(feature = "sound")]
const SONAMES: [&str; 2] = ["libasound.so.2", "libasound.so"];
#[cfg(feature = "sound")]
const PLAYBACK: c_int = 0;
#[cfg(feature = "sound")]
const RW_INTERLEAVED: c_int = 3;
/// 試す順（`snd_pcm_format_t`）。プラグインを通る `default` は浮動小数点のまま受け、ハードウェアを名指しした物は
/// 整数しか受けないことがある。
#[cfg(feature = "sound")]
const FORMATS: [(c_int, Sample); 3] = [
    (14, Sample::FloatLe),
    (10, Sample::S32Le),
    (2, Sample::S16Le),
];
/// デバイスのバッファの長さ（µs）。周期はこれを alsa-lib が割った物。
#[cfg(feature = "sound")]
const LATENCY: c_uint = 100_000;

/// 名前は ALSA の形式（SND_PCM_FORMAT_FLOAT_LE など）に合わせる。
#[cfg(feature = "sound")]
#[derive(Clone, Copy, Debug)]
#[allow(clippy::enum_variant_names)]
enum Sample {
    FloatLe,
    S32Le,
    S16Le,
}

#[cfg(feature = "sound")]
impl Sample {
    fn bytes(self) -> usize {
        match self {
            Self::FloatLe | Self::S32Le => 4,
            Self::S16Le => 2,
        }
    }

    fn encode(self, from: &[f32], into: &mut Vec<u8>) {
        into.clear();
        match self {
            Self::FloatLe => {
                into.extend(from.iter().flat_map(|sample| sample.to_le_bytes()));
            }
            Self::S32Le => into.extend(from.iter().flat_map(|sample| {
                ((f64::from(sample.clamp(-1.0, 1.0)) * f64::from(i32::MAX)) as i32).to_le_bytes()
            })),
            Self::S16Le => into.extend(from.iter().flat_map(|sample| {
                ((sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16).to_le_bytes()
            })),
        }
    }
}

#[cfg(feature = "sound")]
type Handle = *mut c_void;

#[cfg(feature = "sound")]
macro_rules! symbol {
    ($object:expr, $name:literal) => {
        // SAFETY: the name is an entry point of alsa-lib, bound to its signature in `Symbols`.
        unsafe { $object.symbol($name) }
            .map_err(|missing| Error::Unavailable(format!("libasound: {missing}")))?
    };
}

/// alsa-lib の PCM の関数（宣言は `alsa/pcm.h`・`alsa/control.h`・`alsa/error.h`）。
#[cfg(feature = "sound")]
struct Symbols {
    open: unsafe extern "C" fn(*mut Handle, *const c_char, c_int, c_int) -> c_int,
    close: unsafe extern "C" fn(Handle) -> c_int,
    hw_params_malloc: unsafe extern "C" fn(*mut *mut c_void) -> c_int,
    hw_params_free: unsafe extern "C" fn(*mut c_void),
    hw_params_any: unsafe extern "C" fn(Handle, *mut c_void) -> c_int,
    channels_max: unsafe extern "C" fn(*const c_void, *mut c_uint) -> c_int,
    set_params: unsafe extern "C" fn(Handle, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int,
    get_params: unsafe extern "C" fn(Handle, *mut c_ulong, *mut c_ulong) -> c_int,
    writei: unsafe extern "C" fn(Handle, *const c_void, c_ulong) -> c_long,
    delay: unsafe extern "C" fn(Handle, *mut c_long) -> c_int,
    recover: unsafe extern "C" fn(Handle, c_int, c_int) -> c_int,
    drop: unsafe extern "C" fn(Handle) -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
}

#[cfg(feature = "sound")]
impl Symbols {
    fn of(object: &Object) -> Result<Self, Error> {
        Ok(Self {
            open: symbol!(object, b"snd_pcm_open\0"),
            close: symbol!(object, b"snd_pcm_close\0"),
            hw_params_malloc: symbol!(object, b"snd_pcm_hw_params_malloc\0"),
            hw_params_free: symbol!(object, b"snd_pcm_hw_params_free\0"),
            hw_params_any: symbol!(object, b"snd_pcm_hw_params_any\0"),
            channels_max: symbol!(object, b"snd_pcm_hw_params_get_channels_max\0"),
            set_params: symbol!(object, b"snd_pcm_set_params\0"),
            get_params: symbol!(object, b"snd_pcm_get_params\0"),
            writei: symbol!(object, b"snd_pcm_writei\0"),
            delay: symbol!(object, b"snd_pcm_delay\0"),
            recover: symbol!(object, b"snd_pcm_recover\0"),
            drop: symbol!(object, b"snd_pcm_drop\0"),
            strerror: symbol!(object, b"snd_strerror\0"),
        })
    }

    fn say(&self, status: c_int) -> String {
        // SAFETY: returns a static string for any value.
        let text = unsafe { (self.strerror)(status) };
        if text.is_null() {
            return format!("ALSA error {status}");
        }
        // SAFETY: non-null static C string.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }
}

/// 開いた PCM。閉じるのは `Drop` の一度だけ。
#[cfg(feature = "sound")]
struct Device {
    symbols: Symbols,
    pcm: Handle,
    sample: Sample,
    /// 一度に書くフレーム数（周期）。
    period: usize,
    _object: Object,
}

// SAFETY: `snd_pcm_t` はスレッドに結びつかず、渡った先の書き込みのスレッドだけが使う。
#[cfg(feature = "sound")]
unsafe impl Send for Device {}

#[cfg(feature = "sound")]
impl Device {
    /// `name` の PCM を開き、チャンネル数（`channels` が無ければデバイスが受ける最大、8 まで）を決める。
    fn open(name: &str, rate: u32, channels: Option<usize>) -> Result<(Self, usize), Error> {
        let object = Object::open_any(&SONAMES)
            .map_err(|missing| Error::Unavailable(format!("ALSA: {missing}")))?;
        let symbols = Symbols::of(&object)?;
        let text = CString::new(name).map_err(|_| Error::Unsupported("名前に NUL"))?;
        let mut pcm: Handle = std::ptr::null_mut();
        // SAFETY: valid out-pointer; `text` outlives the call.
        let status = unsafe { (symbols.open)(&mut pcm, text.as_ptr(), PLAYBACK, 0) };
        if status < 0 {
            return Err(Error::Unavailable(format!(
                "{name}: {}",
                symbols.say(status)
            )));
        }
        let mut device = Self {
            symbols,
            pcm,
            sample: Sample::FloatLe,
            period: 0,
            _object: object,
        };
        let channels = channels
            .unwrap_or_else(|| device.channels_max())
            .clamp(1, 8);
        // プラグインを通る物は別のレートでも変換して受ける（soft_resample）。
        let mut refused = Vec::new();
        for (format, sample) in FORMATS {
            // SAFETY: `pcm` is open.
            let status = unsafe {
                (device.symbols.set_params)(
                    device.pcm,
                    format,
                    RW_INTERLEAVED,
                    channels as c_uint,
                    rate,
                    1,
                    LATENCY,
                )
            };
            if status >= 0 {
                device.sample = sample;
                device.period = device.period_frames();
                return Ok((device, channels));
            }
            refused.push(format!("{sample:?}: {}", device.symbols.say(status)));
        }
        Err(Error::Unavailable(format!(
            "{name}（{rate} Hz、{channels} チャンネル）を開けない: {}",
            refused.join("、")
        )))
    }

    /// デバイスが受ける最大のチャンネル数。サーバーへ行くプラグイン（`default` など）は何でも受けると答えるので、
    /// 8 を超えたらステレオにする。
    fn channels_max(&self) -> usize {
        let mut params: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid out-pointer.
        if unsafe { (self.symbols.hw_params_malloc)(&mut params) } < 0 || params.is_null() {
            return 2;
        }
        let mut most: c_uint = 0;
        // SAFETY: `pcm` is open and `params` was just allocated; freed once below.
        let found = unsafe {
            (self.symbols.hw_params_any)(self.pcm, params) >= 0
                && (self.symbols.channels_max)(params, &mut most) >= 0
        };
        // SAFETY: allocated above, freed once.
        unsafe { (self.symbols.hw_params_free)(params) };
        match most as usize {
            most @ 1..=8 if found => most,
            _ => 2,
        }
    }

    fn period_frames(&self) -> usize {
        let (mut buffer, mut period): (c_ulong, c_ulong) = (0, 0);
        // SAFETY: `pcm` is open and configured.
        let status = unsafe { (self.symbols.get_params)(self.pcm, &mut buffer, &mut period) };
        if status < 0 || period == 0 {
            return 1024;
        }
        period as usize
    }

    /// 次に書くサンプルが鳴るまでのフレーム数。
    fn delay(&self) -> Option<u64> {
        let mut frames: c_long = 0;
        // SAFETY: `pcm` is open.
        let status = unsafe { (self.symbols.delay)(self.pcm, &mut frames) };
        (status >= 0).then(|| frames.max(0) as u64)
    }

    /// `bytes` を全部書く。途切れ（underrun）とサスペンドからは戻す。
    fn write(&self, bytes: &[u8], frame: usize) -> Result<(), Error> {
        let mut at = 0;
        while at < bytes.len() {
            let frames = (bytes.len() - at) / frame;
            // SAFETY: `bytes[at..]` holds `frames` whole frames.
            let took = unsafe {
                (self.symbols.writei)(self.pcm, bytes[at..].as_ptr().cast(), frames as c_ulong)
            };
            if took < 0 {
                // SAFETY: the handle is open.
                let status = unsafe { (self.symbols.recover)(self.pcm, took as c_int, 1) };
                if status < 0 {
                    return Err(Error::Lost(self.symbols.say(status)));
                }
                continue;
            }
            at += took as usize * frame;
        }
        Ok(())
    }
}

#[cfg(feature = "sound")]
impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: dropped and closed once; nothing waits for what is still buffered.
        unsafe {
            (self.symbols.drop)(self.pcm);
            (self.symbols.close)(self.pcm);
        }
    }
}

/// 書き込みのスレッド。止めて待つのは `Drop`。
pub struct Keep {
    #[cfg(feature = "sound")]
    stop: Arc<AtomicBool>,
    #[cfg(feature = "sound")]
    writer: Option<std::thread::JoinHandle<()>>,
}

impl Keep {
    pub fn none() -> Self {
        Self {
            #[cfg(feature = "sound")]
            stop: Arc::new(AtomicBool::new(true)),
            #[cfg(feature = "sound")]
            writer: None,
        }
    }

    /// 何もしない。既定の `default` は ALSA の設定（とサウンドサーバー）が出力先を決め、こちらで開き直すことは無い。
    pub fn follow_default(&mut self) {}
}

impl Drop for Keep {
    fn drop(&mut self) {
        #[cfg(feature = "sound")]
        {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(writer) = self.writer.take() {
                let _ = writer.join();
            }
        }
    }
}

impl std::fmt::Debug for Keep {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Keep")
    }
}

#[cfg(feature = "sound")]
fn pour(device: &Device, parts: &Parts, stop: &AtomicBool) {
    let channels = usize::from(parts.channels);
    let mut samples = vec![0.0f32; device.period * channels];
    let mut bytes = Vec::with_capacity(samples.len() * device.sample.bytes());
    while !stop.load(Ordering::Relaxed) {
        parts.fill(&mut samples, device.delay());
        device.sample.encode(&samples, &mut bytes);
        if let Err(error) = device.write(&bytes, channels * device.sample.bytes()) {
            tracing::warn!("音声の出力（ALSA）: {error}");
            return;
        }
    }
}

#[cfg(feature = "sound")]
impl Pcm {
    /// デバイスを開く。`channels` が無ければデバイスのチャンネル数で（8 まで）、`named` が無ければ `default` で。
    pub fn open(
        rate: u32,
        channels: Option<usize>,
        named: Option<&str>,
    ) -> Result<(Self, Keep), Error> {
        let named = named.filter(|name| !name.is_empty());
        let (device, channels) = match named {
            Some(name) => Device::open(name, rate, channels).or_else(|error| {
                tracing::warn!("音声の出力先「{name}」を開けない（{error}）。既定のデバイスを使う");
                Device::open("default", rate, channels)
            })?,
            None => Device::open("default", rate, channels)?,
        };
        tracing::info!(
            "音声の出力: ALSA {}（{rate} Hz、{channels} チャンネル、{:?}、周期 {} フレーム）",
            named.unwrap_or("default"),
            device.sample,
            device.period
        );
        let parts = Parts::new(rate, channels);
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let parts = parts.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name(String::from("alsa"))
                .spawn(move || pour(&device, &parts, &stop))
                .map_err(|error| Error::Unavailable(error.to_string()))?
        };
        Ok((
            parts.pcm(),
            Keep {
                stop,
                writer: Some(writer),
            },
        ))
    }
}

/// 出力に使える PCM の名前（`snd_device_name_hint` の `NAME`）。設定でこの名前を選ぶとそのまま開く。
#[cfg(feature = "sound")]
pub fn output_names() -> Vec<String> {
    hinted().unwrap_or_default()
}

#[cfg(feature = "sound")]
fn hinted() -> Option<Vec<String>> {
    type Hint = unsafe extern "C" fn(c_int, *const c_char, *mut *mut *mut c_void) -> c_int;
    type Get = unsafe extern "C" fn(*const c_void, *const c_char) -> *mut c_char;
    type Free = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
    unsafe extern "C" {
        fn free(pointer: *mut c_void);
    }
    let object = Object::open_any(&SONAMES).ok()?;
    // SAFETY: entry points of alsa-lib bound to their signatures in `alsa/control.h`.
    let (hint, get, release) = unsafe {
        (
            object.symbol::<Hint>(b"snd_device_name_hint\0").ok()?,
            object.symbol::<Get>(b"snd_device_name_get_hint\0").ok()?,
            object.symbol::<Free>(b"snd_device_name_free_hint\0").ok()?,
        )
    };
    let take = |hint: *const c_void, id: &CStr| -> Option<String> {
        // SAFETY: `hint` is an entry of the list below; the returned string is ours to free.
        let text = unsafe { get(hint, id.as_ptr()) };
        if text.is_null() {
            return None;
        }
        // SAFETY: non-null C string from alsa-lib, freed once right after.
        let owned = unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: allocated by alsa-lib with malloc.
        unsafe { free(text.cast()) };
        Some(owned)
    };
    let mut hints: *mut *mut c_void = std::ptr::null_mut();
    // SAFETY: valid out-pointer; card -1 is every card.
    if unsafe { hint(-1, c"pcm".as_ptr(), &mut hints) } < 0 || hints.is_null() {
        return None;
    }
    let mut names = Vec::new();
    let mut at = 0;
    loop {
        // SAFETY: the list ends with a null entry.
        let entry = unsafe { *hints.add(at) };
        if entry.is_null() {
            break;
        }
        at += 1;
        // 入力だけの物は除く（`IOID` が無ければ両方）。
        if take(entry, c"IOID").is_some_and(|io| io != "Output") {
            continue;
        }
        if let Some(name) = take(entry, c"NAME")
            && name != "null"
        {
            names.push(name);
        }
    }
    // SAFETY: the list from `snd_device_name_hint`, freed once.
    unsafe { release(hints) };
    Some(names)
}

#[cfg(not(feature = "sound"))]
pub fn output_names() -> Vec<String> {
    Vec::new()
}

/// 音声の出力方式の名前。
pub fn host_name() -> String {
    if cfg!(feature = "sound") {
        String::from("ALSA")
    } else {
        String::from("なし")
    }
}
