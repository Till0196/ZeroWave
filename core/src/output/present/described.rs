// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Wayland のウィンドウの `wl_surface` に、EGL で描いた中身の色空間（BT.2020 の PQ・HLG）を伝える
//! （`wp_color_management_v1`）。Mesa の EGL は Wayland でもサーフェスの色空間に sRGB とリニアしか
//! 受け付けないので、10 ビットのサーフェスに符号化した値を書き、こちらから合成側へ伝える。
//! 送るのは合成側が通知した物だけ（ほかを送ると接続ごと切られる）。

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::color_management::v1::client::{
    wp_color_management_surface_v1::WpColorManagementSurfaceV1,
    wp_color_manager_v1::{self, Feature, RenderIntent, TransferFunction, WpColorManagerV1},
    wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
    wp_image_description_v1::{self, WpImageDescriptionV1},
};

use crate::output::video::Space;

#[derive(Debug, Default)]
struct State {
    intents: Vec<u32>,
    features: Vec<u32>,
    curves: Vec<u32>,
    gamuts: Vec<u32>,
    described: Option<Result<(), String>>,
}

#[derive(Debug)]
pub struct Described {
    connection: Connection,
    queue: EventQueue<State>,
    handle: QueueHandle<State>,
    state: State,
    manager: WpColorManagerV1,
    window: WlSurface,
    surface: Option<WpColorManagementSurfaceV1>,
}

impl Described {
    /// 合成側が BT.2020 の PQ か HLG をパラメーターで受けられなければ `Err`。
    /// # Safety
    /// `display` と `surface` は開いている `wl_display *` とその接続の `wl_surface *` で、
    /// 返った物より長く生きること。
    pub unsafe fn new(
        display: *mut std::ffi::c_void,
        surface: *mut std::ffi::c_void,
    ) -> Result<Self, String> {
        // SAFETY: 呼び出し側が `display` を約束している。
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let connection = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<State>(&connection)
            .map_err(|error| format!("グローバルを読めません: {error}"))?;
        let handle = queue.handle();
        let manager: WpColorManagerV1 = globals
            .bind(&handle, 1..=2, ())
            .map_err(|_| String::from("wp_color_management_v1 がありません"))?;
        // SAFETY: 呼び出し側が `surface` を約束している。winit の `wl_surface` を借用する。
        let window = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }
            .ok()
            .and_then(|id| WlSurface::from_id(&connection, id).ok())
            .ok_or_else(|| String::from("ウィンドウの wl_surface を取得できません"))?;
        let mut state = State::default();
        queue
            .roundtrip(&mut state)
            .map_err(|error| format!("対応の一覧を受け取れません: {error}"))?;
        let described = Self {
            connection,
            queue,
            handle,
            state,
            manager,
            window,
            surface: None,
        };
        if !described.offers(Space::Pq) && !described.offers(Space::Hlg) {
            return Err(format!(
                "BT.2020 の PQ も HLG も伝えられません（曲線 {:?}、色域 {:?}、機能 {:?}）",
                described.state.curves, described.state.gamuts, described.state.features
            ));
        }
        Ok(described)
    }

    pub fn offers(&self, space: Space) -> bool {
        let Some(curve) = curve(space) else {
            return space == Space::Srgb;
        };
        let state = &self.state;
        state.features.contains(&(Feature::Parametric as u32))
            && state.intents.contains(&(RenderIntent::Perceptual as u32))
            && state.curves.contains(&(curve as u32))
            && state
                .gamuts
                .contains(&(wp_color_manager_v1::Primaries::Bt2020 as u32))
    }

    /// 次の `commit`（`eglSwapBuffers`）で効く。sRGB なら記述を外す（記述の無いサーフェスは sRGB）。
    pub fn tell(&mut self, space: Space) -> Result<(), String> {
        let Some(curve) = curve(space) else {
            if let Some(surface) = &self.surface {
                surface.unset_image_description();
            }
            return self.flush();
        };
        if !self.offers(space) {
            return Err(format!("{space:?} を伝えられません"));
        }
        let params = self.manager.create_parametric_creator(&self.handle, ());
        params.set_tf_named(curve);
        params.set_primaries_named(wp_color_manager_v1::Primaries::Bt2020);
        self.state.described = None;
        let description = params.create(&self.handle, ());
        // `ready` か `failed` が来るまで待つ（ready でない記述を付けると接続が切られる）。
        for _ in 0..4 {
            if self.state.described.is_some() {
                break;
            }
            if let Err(error) = self.queue.roundtrip(&mut self.state) {
                description.destroy();
                return Err(format!("記述への応答を受け取れません: {error}"));
            }
        }
        let outcome = self
            .state
            .described
            .take()
            .unwrap_or_else(|| Err(String::from("合成側が記述に応答しません")));
        if outcome.is_ok() {
            let surface = self
                .surface
                .get_or_insert_with(|| self.manager.get_surface(&self.window, &self.handle, ()));
            surface.set_image_description(&description, RenderIntent::Perceptual);
        }
        description.destroy();
        outcome?;
        self.flush()
    }

    /// EGL の `commit` より先に届くよう、すぐ送る（同じ接続なので順は保たれる）。
    fn flush(&self) -> Result<(), String> {
        self.connection
            .flush()
            .map_err(|error| format!("送れません: {error}"))
    }
}

impl Drop for Described {
    fn drop(&mut self) {
        if let Some(surface) = self.surface.take() {
            surface.destroy();
        }
        self.manager.destroy();
        let _ = self.connection.flush();
    }
}

fn curve(space: Space) -> Option<TransferFunction> {
    match space {
        Space::Pq => Some(TransferFunction::St2084Pq),
        Space::Hlg => Some(TransferFunction::Hlg),
        Space::Srgb | Space::ScRgb => None,
    }
}

fn number<T: Into<u32>>(value: WEnum<T>) -> u32 {
    match value {
        WEnum::Value(value) => value.into(),
        WEnum::Unknown(value) => value,
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

impl Dispatch<WpColorManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_color_manager_v1::Event;
        match event {
            Event::SupportedIntent { render_intent } => state.intents.push(number(render_intent)),
            Event::SupportedFeature { feature } => state.features.push(number(feature)),
            Event::SupportedTfNamed { tf } => state.curves.push(number(tf)),
            Event::SupportedPrimariesNamed { primaries } => state.gamuts.push(number(primaries)),
            _ => {}
        }
    }
}

impl Dispatch<WpImageDescriptionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpImageDescriptionV1,
        event: wp_image_description_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_image_description_v1::Event;
        match event {
            Event::Ready { .. } | Event::Ready2 { .. } => state.described = Some(Ok(())),
            Event::Failed { msg, .. } => state.described = Some(Err(msg)),
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WpColorManagementSurfaceV1);
delegate_noop!(State: ignore WpImageDescriptionCreatorParamsV1);
