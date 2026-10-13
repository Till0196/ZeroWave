// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! WASAPI in exclusive mode: shared mode mixes, which cannot be done to a data burst.
//! The subtype is written out here because it is in `ksmedia.h`, not the Win32 metadata.

use std::sync::Arc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_UNSUPPORTED_FORMAT,
    AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE_ACTIVE,
    IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::core::GUID;

use super::pump::{FRAME, Pump};

/// 閉じるかを見る間隔でもある。
const WAIT_MOST_MS: u32 = 200;
use super::{Error, Link};

/// CEA-861 type 6.
const SUBTYPE_IEC61937_AAC: GUID = GUID::from_u128(0x0000_0006_0cea_0010_8000_00aa_0038_9b71);
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
/// Advisory: a receiver told 5.1 and handed stereo plays stereo, so the larger is safe.
const SPEAKER_5POINT1: u32 = 0x3f;
/// In hundreds of nanoseconds.
const BUFFER: i64 = 2_000_000;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Iec61937Format {
    format: WAVEFORMATEXTENSIBLE,
    encoded_rate: u32,
    encoded_channels: u32,
    average_bytes: u32,
}

/// Documented as 52 bytes with `cbSize` 34; otherwise the trailing words are read wrongly.
const _: () = {
    assert!(size_of::<Iec61937Format>() == 52);
    assert!(size_of::<WAVEFORMATEX>() == 18);
};

impl Iec61937Format {
    fn new(rate: u32) -> Self {
        Self {
            format: WAVEFORMATEXTENSIBLE {
                Format: WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                    nChannels: 2,
                    nSamplesPerSec: rate,
                    nAvgBytesPerSec: rate * FRAME as u32,
                    nBlockAlign: FRAME as u16,
                    wBitsPerSample: 16,
                    // Not the 22 of the difference: 34 tells Windows to read the three words after.
                    cbSize: 34,
                },
                Samples: WAVEFORMATEXTENSIBLE_0 {
                    wValidBitsPerSample: 16,
                },
                dwChannelMask: SPEAKER_5POINT1,
                SubFormat: SUBTYPE_IEC61937_AAC,
            },
            encoded_rate: rate,
            encoded_channels: 6,
            average_bytes: 0,
        }
    }

    fn as_wave(&self) -> *const WAVEFORMATEX {
        (&raw const self.format.Format).cast()
    }
}

struct Session {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: HANDLE,
    frames: u32,
}

// SAFETY: created in the multithreaded apartment and used only by the writing thread.
unsafe impl Send for Session {}

#[derive(Debug)]
pub struct Wasapi {
    pump: Arc<Pump>,
    writer: Option<JoinHandle<()>>,
    rate: u32,
}

impl Wasapi {
    /// `on` is part of an endpoint's name, matched without case; `None` is the default.
    pub fn open(rate: u32, on: Option<&str>) -> Result<Self, Error> {
        let on = on.map(str::to_owned);
        let pump = Arc::new(Pump::new());
        let theirs = pump.clone();
        // Built on the thread that uses it: COM objects belong to apartments.
        let (ready, heard) = std::sync::mpsc::channel();
        let writer = std::thread::Builder::new()
            .name(String::from("zerowave-wasapi"))
            .spawn(move || {
                let session = match Session::open(rate, on.as_deref()) {
                    Ok(session) => {
                        let _ = ready.send(Ok(()));
                        session
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                pour(&session, &theirs);
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

impl Session {
    fn open(rate: u32, on: Option<&str>) -> Result<Self, Error> {
        // SAFETY: called once per thread before anything else COM. MTA: no message pump here.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(failed)?;

        let device = endpoint(&enumerator()?, on)?;
        let client = activate(&device)?;

        let format = Iec61937Format::new(rate);
        // SAFETY: the format outlives the call, which only reads it.
        let supported = unsafe {
            client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, format.as_wave(), None)
        };
        if supported.is_err() {
            return Err(Error::Unavailable(String::from(
                "この出力デバイスは AAC のパススルーに対応しない（レシーバーの EDID か、Windows の「サウンド」の設定）",
            )));
        }

        let (client, frames) = start(&device, client, &format, BUFFER)?;
        // SAFETY: the ordinary unnamed auto-reset event.
        let event = unsafe { CreateEventW(None, false, false, None) }.map_err(failed)?;
        // ここから先で失敗したら、イベントを閉じてから抜ける。
        let started = (|| -> Result<IAudioRenderClient, Error> {
            // SAFETY: the client is initialised and the event is ours.
            unsafe { client.SetEventHandle(event) }.map_err(failed)?;
            // SAFETY: the client is initialised for rendering.
            let render: IAudioRenderClient = unsafe { client.GetService() }.map_err(failed)?;
            // SAFETY: as above.
            unsafe { client.Start() }.map_err(failed)?;
            Ok(render)
        })();
        let render = match started {
            Ok(render) => render,
            Err(error) => {
                // SAFETY: the event was made just above and nothing else holds it.
                let _ = unsafe { CloseHandle(event) };
                return Err(error);
            }
        };

        Ok(Self {
            client,
            render,
            event,
            frames,
        })
    }
}

/// An exclusive buffer has to be a whole number of device periods, learnt only from
/// `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`; then the client is made again.
fn start(
    device: &windows::Win32::Media::Audio::IMMDevice,
    client: IAudioClient,
    format: &Iec61937Format,
    duration: i64,
) -> Result<(IAudioClient, u32), Error> {
    match initialize(&client, format, duration) {
        Ok(()) => {
            let frames = buffer_size(&client)?;
            Ok((client, frames))
        }
        Err(error) if error.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
            // 拒否した client も希望するバッファサイズは返す。
            let frames = buffer_size(&client)?;
            let rate = format.format.Format.nSamplesPerSec;
            let wanted = (10_000_000.0 * f64::from(frames) / f64::from(rate)).round() as i64;
            // 一度 Initialize した client はやり直せないので、同じデバイスから作り直す。
            let again = activate(device)?;
            initialize(&again, format, wanted).map_err(failed)?;
            let frames = buffer_size(&again)?;
            Ok((again, frames))
        }
        Err(error) if error.code() == AUDCLNT_E_DEVICE_IN_USE => Err(Error::Unavailable(
            String::from("この出力デバイスは他のプログラムが排他モードで使っている"),
        )),
        Err(error) if error.code() == AUDCLNT_E_UNSUPPORTED_FORMAT => Err(Error::Unavailable(
            String::from("この出力デバイスは AAC のパススルーに対応しない"),
        )),
        Err(error) => Err(failed(error)),
    }
}

/// COM に入ったスレッドで呼ぶ（入っていなければ失敗が返る）。
fn enumerator() -> Result<IMMDeviceEnumerator, Error> {
    // SAFETY: 組になったクラスとインターフェースを要求するだけ。
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(failed)
}

fn default_endpoint(
    devices: &IMMDeviceEnumerator,
) -> Result<windows::Win32::Media::Audio::IMMDevice, Error> {
    // SAFETY: 生きている列挙子と、決まった二つの値。
    unsafe { devices.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(failed)
}

fn activate(device: &windows::Win32::Media::Audio::IMMDevice) -> Result<IAudioClient, Error> {
    // SAFETY: 生きているデバイスに、決まったインターフェースを要求する。
    unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(failed)
}

fn initialize(
    client: &IAudioClient,
    format: &Iec61937Format,
    duration: i64,
) -> windows::core::Result<()> {
    // SAFETY: フォーマットは呼ぶ間だけ読まれる。
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_EXCLUSIVE,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            duration,
            duration,
            format.as_wave(),
            None,
        )
    }
}

fn buffer_size(client: &IAudioClient) -> Result<u32, Error> {
    // SAFETY: 引数の無い問い合わせ。
    unsafe { client.GetBufferSize() }.map_err(failed)
}

fn endpoint(
    devices: &IMMDeviceEnumerator,
    on: Option<&str>,
) -> Result<windows::Win32::Media::Audio::IMMDevice, Error> {
    let Some(on) = on else {
        return default_endpoint(devices);
    };
    let wanted = on.to_lowercase();
    // SAFETY: as above.
    let all =
        unsafe { devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }.map_err(failed)?;
    // SAFETY: the collection is live.
    let count = unsafe { all.GetCount() }.map_err(failed)?;
    let mut names = Vec::new();
    for at in 0..count {
        // SAFETY: `at` is inside the collection.
        let device = unsafe { all.Item(at) }.map_err(failed)?;
        let name = friendly(&device).unwrap_or_default();
        if name.to_lowercase().contains(&wanted) {
            return Ok(device);
        }
        names.push(name);
    }
    Err(Error::Unavailable(format!(
        "「{on}」に一致する出力デバイスが無い。あるのは: {}",
        names.join(", ")
    )))
}

fn friendly(device: &windows::Win32::Media::Audio::IMMDevice) -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;

    // SAFETY: the device is live and the store is opened for reading only.
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    // SAFETY: the key is a well known one and the store is live.
    let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    // SAFETY: the tag (`VT_LPWSTR`) is checked first; the string lives as long as the value.
    unsafe {
        let held = &value.Anonymous.Anonymous;
        if held.vt != VT_LPWSTR {
            return None;
        }
        held.Anonymous.pwszVal.to_string().ok()
    }
}

fn pour(session: &Session, pump: &Pump) {
    let want = session.frames as usize * FRAME;
    let mut held = Vec::new();
    // Exclusive mode always says full, but the device takes one whole buffer a turn, so count it.
    let mut real: u64 = 0;
    while !pump.closing() {
        // SAFETY: the event was made here and the client signals it.
        // 永久には待たない。デバイスが抜かれるとイベントが来ず、`Drop` の join まで戻れなくなる。
        let waited = unsafe { WaitForSingleObject(session.event, WAIT_MOST_MS) };
        if waited == WAIT_TIMEOUT {
            continue;
        }
        if waited.0 != 0 {
            tracing::warn!("音声（WASAPI）: 待機に失敗 ({:?})", waited.0);
            break;
        }
        if pump.closing() {
            break;
        }
        pump.take(want, &mut held);

        // SAFETY: signalled for exactly this many frames; ours until `ReleaseBuffer`.
        let into = match unsafe { session.render.GetBuffer(session.frames) } {
            Ok(into) => into,
            Err(error) => {
                tracing::warn!("音声（WASAPI）: {error}");
                break;
            }
        };
        // SAFETY: the buffer holds `want` bytes and `held.len() <= want`.
        unsafe {
            std::ptr::copy_nonoverlapping(held.as_ptr(), into, held.len());
            std::ptr::write_bytes(into.add(held.len()), 0, want - held.len());
            // SAFETY: the whole buffer is released, as exclusive mode with an event requires.
            let _ = session.render.ReleaseBuffer(session.frames, 0);
        }
        let written = (held.len() / FRAME) as u64;
        pump.handed(written);
        real = real.saturating_sub(u64::from(session.frames)) + written;
        pump.holding(real);
    }
    // SAFETY: the client was started and is stopped once.
    unsafe {
        let _ = session.client.Stop();
        let _ = CloseHandle(session.event);
    }
}

fn failed(error: windows::core::Error) -> Error {
    Error::Unavailable(error.to_string())
}

impl Link for Wasapi {
    fn write(&mut self, burst: &[u8]) -> Result<usize, Error> {
        self.pump.push(burst);
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
        "WASAPI"
    }
}

impl Drop for Wasapi {
    fn drop(&mut self) {
        self.pump.close();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}
