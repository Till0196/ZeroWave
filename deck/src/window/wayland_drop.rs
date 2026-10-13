// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Wayland でウィンドウへドロップされたファイルを受け取る（winit は対応しないので `wl_data_device` を自分で持つ）。
//! 接続は winit の物を借り、イベントキューだけ自分で持つ。

use std::io::Read;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::{DndAction, WlDataDeviceManager},
    wl_data_offer::{self, WlDataOffer},
    wl_registry::WlRegistry,
    wl_seat::WlSeat,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};

const URI_LIST: &str = "text/uri-list";

const PATIENCE: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
struct State {
    surface: Option<ObjectId>,
    offers: Vec<(WlDataOffer, Vec<String>)>,
    over: Option<WlDataOffer>,
    /// 読み終えたファイル（読むのは別のスレッド）。
    dropped: Option<std::sync::mpsc::Sender<Vec<PathBuf>>>,
}

pub struct WaylandDrop {
    queue: EventQueue<State>,
    state: State,
    dropped: std::sync::mpsc::Receiver<Vec<PathBuf>>,
    _device: WlDataDevice,
}

impl std::fmt::Debug for WaylandDrop {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("WaylandDrop").finish_non_exhaustive()
    }
}

impl WaylandDrop {
    pub fn of(window: &winit::window::Window) -> Option<Self> {
        use winit::raw_window_handle::{
            HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
        };
        let RawDisplayHandle::Wayland(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(surface) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: winit の `wl_display *` と `wl_surface *`。どちらもプロセスの終わりまで生きる。
        let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let (globals, mut queue) = match registry_queue_init::<State>(&connection) {
            Ok(both) => both,
            Err(error) => {
                tracing::info!("Wayland のドロップ: グローバルを読めない: {error}");
                return None;
            }
        };
        let handle = queue.handle();
        let Ok(manager) = globals.bind::<WlDataDeviceManager, _, _>(&handle, 1..=3, ()) else {
            tracing::info!("Wayland のドロップ: wl_data_device_manager が無い");
            return None;
        };
        let Ok(seat) = globals.bind::<WlSeat, _, _>(&handle, 1..=1, ()) else {
            tracing::info!("Wayland のドロップ: wl_seat が無い");
            return None;
        };
        let device = manager.get_data_device(&seat, &handle, ());
        // SAFETY: 上と同じ、winit の `wl_surface *`。
        let surface =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.surface.as_ptr().cast()) }
                .ok();
        let (into, dropped) = std::sync::mpsc::channel();
        let mut state = State {
            surface,
            dropped: Some(into),
            ..State::default()
        };
        if let Err(error) = queue.roundtrip(&mut state) {
            tracing::info!("Wayland のドロップ: バインドできない: {error}");
            return None;
        }
        Some(Self {
            queue,
            state,
            dropped,
            _device: device,
        })
    }

    pub fn take(&mut self) -> Vec<PathBuf> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        self.dropped.try_iter().flatten().collect()
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore WlDataDeviceManager);

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        state: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Some((_, kinds)) = state.offers.iter_mut().find(|(one, _)| one == offer)
        {
            kinds.push(mime_type);
        }
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        connection: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                // ウィンドウの上に無いオファー（選ばれなかった物・クリップボードの物）は捨てる。
                let over = state.over.clone();
                state.offers.retain(|(one, _)| {
                    let keep = over.as_ref() == Some(one);
                    if !keep {
                        one.destroy();
                    }
                    keep
                });
                state.offers.push((id, Vec::new()));
            }
            // クリップボードは egui の方が受ける。こちらのオファーは要らない。
            wl_data_device::Event::Selection { id: Some(offer) } => {
                offer.destroy();
                state.offers.retain(|(one, _)| *one != offer);
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                id,
                ..
            } => {
                state.over = None;
                let Some(offer) = id else {
                    return;
                };
                let ours = state.surface.as_ref() == Some(&surface.id());
                let files = state
                    .offers
                    .iter()
                    .any(|(one, kinds)| *one == offer && kinds.iter().any(|kind| kind == URI_LIST));
                if ours && files {
                    offer.accept(serial, Some(String::from(URI_LIST)));
                    if offer.version() >= 3 {
                        offer.set_actions(DndAction::Copy, DndAction::Copy);
                    }
                    state.over = Some(offer);
                } else {
                    offer.accept(serial, None);
                }
            }
            wl_data_device::Event::Leave => state.over = None,
            wl_data_device::Event::Drop => {
                let Some(offer) = state.over.take() else {
                    return;
                };
                state.offers.retain(|(one, _)| *one != offer);
                if let Some(into) = state.dropped.clone() {
                    read_uris(offer, connection.clone(), into);
                }
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

/// オファーを読む。ウィンドウのスレッドを止めないよう、読み込みと後始末は別のスレッドで。
fn read_uris(
    offer: WlDataOffer,
    connection: Connection,
    into: std::sync::mpsc::Sender<Vec<PathBuf>>,
) {
    let pipe = std::io::pipe();
    let Ok((reader, writer)) = pipe else {
        offer.destroy();
        return;
    };
    offer.receive(String::from(URI_LIST), writer.as_fd());
    drop(writer);
    let _ = connection.flush();
    let spawned = std::thread::Builder::new()
        .name(String::from("wayland drop"))
        .spawn(move || {
            match read_within(reader, PATIENCE) {
                Ok(text) => {
                    let _ = into.send(paths_of(&text));
                }
                Err(error) => tracing::info!("Wayland のドロップを読めない: {error}"),
            }
            if offer.version() >= 3 {
                offer.finish();
            }
            offer.destroy();
            let _ = connection.flush();
        });
    if let Err(error) = spawned {
        tracing::info!("Wayland のドロップを読むスレッドを起動できない: {error}");
    }
}

/// 読む量の上限。ファイルの一覧なので十分。
const MOST: usize = 1 << 20;

fn read_within(mut reader: std::io::PipeReader, patience: Duration) -> std::io::Result<String> {
    use std::os::fd::AsRawFd;
    let until = Instant::now() + patience;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut poll = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: 一つの pollfd と、その数。
        let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis().min(1000) as i32) };
        if ready < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if ready == 0 {
            continue;
        }
        match reader.read(&mut chunk)? {
            0 => break,
            read => bytes.extend_from_slice(&chunk[..read]),
        }
        if bytes.len() > MOST {
            bytes.truncate(MOST);
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `text/uri-list`（RFC 2483）の `file:` の行をパスに。
fn paths_of(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let rest = line.strip_prefix("file://")?;
            let path = &rest[rest.find('/')?..];
            Some(PathBuf::from(
                <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&unescape(path))
                    .to_owned(),
            ))
        })
        .collect()
}

fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(hex) = text.get(at + 1..at + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_list_becomes_paths() {
        let text = "# from the file manager\r\nfile:///home/me/%E9%8C%B2%E7%94%BB%201.ts\r\nfile://host/tmp/b.m2ts\r\nhttps://example.com/x.ts\r\n";
        assert_eq!(
            paths_of(text),
            [
                PathBuf::from("/home/me/録画 1.ts"),
                PathBuf::from("/tmp/b.m2ts")
            ]
        );
    }
}
