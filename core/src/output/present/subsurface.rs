// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Wayland のウィンドウで、映像をサブサーフェス（`wl_subsurface`）に DMA-BUF のまま載せる。親は映像の所を透明に
//! して OSD をレンダリングする。要求はコンポジターが通知した物しか送らない（ほかを送ると接続ごと切られる）。

use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::WlCompositor,
    wl_region::WlRegion,
    wl_registry::WlRegistry,
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::color_management::v1::client::{
    wp_color_management_surface_v1::WpColorManagementSurfaceV1,
    wp_color_manager_v1::{self, WpColorManagerV1},
    wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
    wp_image_description_v1::{self, WpImageDescriptionV1},
};
use wayland_protocols::wp::color_representation::v1::client::{
    wp_color_representation_manager_v1::{self, WpColorRepresentationManagerV1},
    wp_color_representation_surface_v1::{self, WpColorRepresentationSurfaceV1},
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use super::import::FieldView;
use crate::decode::DmaBuf;
use crate::decode::takes::without_parameter;
use crate::output::video::Error;
use crate::output::video::field::Parity;
use crate::output::video::{Matrix, Primaries, Settings, Transfer};

/// コンポジターが返した `wl_buffer` と、コンポジターが通知した物を集める。
#[derive(Debug, Default)]
struct State {
    /// 次にイベントを処理したときに破棄する。
    released: Arc<Mutex<Vec<WlBuffer>>>,
    formats: Vec<(u32, u64)>,
    intents: Vec<u32>,
    features: Vec<u32>,
    curves: Vec<u32>,
    gamuts: Vec<u32>,
    matrices: Vec<(u32, u32)>,
    described: Option<Result<(), String>>,
}

/// サブサーフェスで出せるかと、コンポジターへ伝える中身を決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub transfer: Transfer,
    pub primaries: Primaries,
    pub matrix: Matrix,
    pub limited: bool,
}

impl Look {
    pub fn of(settings: &Settings) -> Self {
        Self {
            transfer: settings.transfer,
            primaries: settings.primaries,
            matrix: settings.matrix,
            limited: settings.limited,
        }
    }

    /// SDR の BT.709。記述の無いサーフェスは sRGB として扱われる。
    fn plain(&self) -> bool {
        self.transfer == Transfer::Sdr
            && self.primaries == Primaries::Bt709
            && self.matrix == Matrix::Bt709
    }

    fn curve(&self) -> Option<wp_color_manager_v1::TransferFunction> {
        use wp_color_manager_v1::TransferFunction;
        match self.transfer {
            Transfer::Sdr => Some(TransferFunction::Bt1886),
            Transfer::Hlg => Some(TransferFunction::Hlg),
            Transfer::Pq => Some(TransferFunction::St2084Pq),
            Transfer::Ui => None,
        }
    }

    fn gamut(&self) -> wp_color_manager_v1::Primaries {
        match self.primaries {
            Primaries::Bt709 => wp_color_manager_v1::Primaries::Srgb,
            Primaries::Bt2020 => wp_color_manager_v1::Primaries::Bt2020,
        }
    }

    fn coefficients(&self) -> wp_color_representation_surface_v1::Coefficients {
        match self.matrix {
            Matrix::Bt709 => wp_color_representation_surface_v1::Coefficients::Bt709,
            Matrix::Bt2020 => wp_color_representation_surface_v1::Coefficients::Bt2020,
        }
    }

    fn range(&self) -> wp_color_representation_surface_v1::Range {
        if self.limited {
            wp_color_representation_surface_v1::Range::Limited
        } else {
            wp_color_representation_surface_v1::Range::Full
        }
    }
}

fn number<T: Into<u32>>(value: WEnum<T>) -> u32 {
    match value {
        WEnum::Value(value) => value.into(),
        WEnum::Unknown(raw) => raw,
    }
}

#[derive(Debug)]
pub struct VideoSurface {
    connection: Connection,
    queue: EventQueue<State>,
    handle: QueueHandle<State>,
    state: State,
    dmabuf: ZwpLinuxDmabufV1,
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    /// 次を付けたら、コンポジターが返したときに破棄する。
    shown: Option<WlBuffer>,
    placed: Option<(i32, i32, i32, i32)>,
    /// フレームなら一つ、フィールドだけの bob なら上と下の二つ。
    slots: Vec<Vec<WlBuffer>>,
    sizes: Vec<Option<(u32, u32)>>,
    source: Option<(f64, f64, f64, f64)>,
    color_manager: Option<WpColorManagerV1>,
    representation_manager: Option<WpColorRepresentationManagerV1>,
    color_surface: Option<WpColorManagementSurfaceV1>,
    representation_surface: Option<WpColorRepresentationSurfaceV1>,
    told: Option<(Look, bool)>,
}

impl VideoSurface {
    /// 要る物が無ければ `Err`。
    /// # Safety
    /// `display` と `parent` は開いている `wl_display *` とその接続の `wl_surface *` で、
    /// 返った物より長く生きること。
    pub unsafe fn new(
        display: *mut std::ffi::c_void,
        parent: *mut std::ffi::c_void,
    ) -> Result<Self, Error> {
        let failed = |what: &str| Error::NoDevice(format!("Wayland: {what}"));
        // SAFETY: 呼び出し側が `display` を約束している。
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&connection)
            .map_err(|error| failed(&format!("グローバルを読めません: {error}")))?;
        let handle = queue.handle();
        let compositor: WlCompositor = globals
            .bind(&handle, 4..=6, ())
            .map_err(|_| failed("wl_compositor がありません"))?;
        let subcompositor: WlSubcompositor = globals
            .bind(&handle, 1..=1, ())
            .map_err(|_| failed("wl_subcompositor がありません"))?;
        // `modifier` は 3 までしか来ないので 3 でバインドする（`create_immed` は 2 から）。
        let dmabuf: ZwpLinuxDmabufV1 = globals
            .bind(&handle, 3..=3, ())
            .map_err(|_| failed("zwp_linux_dmabuf_v1（3 以上）がありません"))?;
        let color_manager: Option<WpColorManagerV1> = globals.bind(&handle, 1..=2, ()).ok();
        let representation_manager: Option<WpColorRepresentationManagerV1> =
            globals.bind(&handle, 1..=1, ()).ok();
        let viewporter: WpViewporter = globals
            .bind(&handle, 1..=1, ())
            .map_err(|_| failed("wp_viewporter がありません"))?;
        // SAFETY: 呼び出し側が `parent` を約束している。winit の `wl_surface` を借用する。
        let parent = unsafe { ObjectId::from_ptr(WlSurface::interface(), parent.cast()) }
            .and_then(|id| WlSurface::from_id(&connection, id))
            .map_err(|_| failed("親の wl_surface を取得できません"))?;
        let surface = compositor.create_surface(&handle, ());
        let subsurface = subcompositor.get_subsurface(&surface, &parent, &handle, ());
        subsurface.place_below(&parent);
        subsurface.set_desync();
        // 入力は親が受ける（サブサーフェスの上でクリックしてもウィンドウの操作になるように）。
        let empty: WlRegion = compositor.create_region(&handle, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        let viewport = viewporter.get_viewport(&surface, &handle, ());
        let mut queue = queue;
        let mut state = State::default();
        queue
            .roundtrip(&mut state)
            .map_err(|error| failed(&format!("対応形式の一覧を受け取れません: {error}")))?;
        let color = match &color_manager {
            Some(_) => format!("あり（曲線 {:?}、色域 {:?}）", state.curves, state.gamuts),
            None => String::from("なし"),
        };
        let representation = match &representation_manager {
            Some(_) => format!("あり（行列と範囲 {:?}）", state.matrices),
            None => String::from("なし"),
        };
        tracing::info!(
            "Wayland: サブサーフェスの DMA-BUF 形式 {} 種類、カラーマネジメント {color}、色表現 {representation}",
            state.formats.len()
        );
        Ok(Self {
            connection,
            queue,
            handle,
            state,
            dmabuf,
            surface,
            subsurface,
            viewport,
            shown: None,
            placed: None,
            slots: Vec::new(),
            sizes: Vec::new(),
            source: None,
            color_manager,
            representation_manager,
            color_surface: None,
            representation_surface: None,
            told: None,
        })
    }

    /// 出せるなら色の情報を伝えておく（次の `commit` で効く）。
    pub fn takes(&mut self, frame: &DmaBuf, look: Look) -> bool {
        let Some(fourcc) = frame.fourcc() else {
            return false;
        };
        let listed = self.state.formats.iter().any(|&(format, modifier)| {
            format == fourcc && without_parameter(modifier) == without_parameter(frame.modifier)
        });
        // 8 ビットの SDR は何も伝えない。
        if look.plain() && fourcc == DmaBuf::NV12 {
            if !listed && !self.state.formats.is_empty() {
                return false;
            }
            self.untell();
            return true;
        }
        if !listed {
            return false;
        }
        match self.told {
            Some((told, ok)) if told == look => ok,
            _ => {
                let ok = match self.describe(look) {
                    Ok(()) => {
                        tracing::info!(
                            "Wayland: {look:?} をサブサーフェスで表示する（色変換はコンポジター）"
                        );
                        true
                    }
                    Err(why) => {
                        tracing::info!(
                            "Wayland: {look:?} はサブサーフェスで表示できないので EGL でレンダリングする（{why}）"
                        );
                        false
                    }
                };
                self.told = Some((look, ok));
                ok
            }
        }
    }

    fn untell(&mut self) {
        if self.told.take().is_some_and(|(_, ok)| ok) {
            if let Some(surface) = &self.color_surface {
                surface.unset_image_description();
            }
            if let Some(surface) = self.representation_surface.take() {
                surface.destroy();
            }
        }
    }

    /// コンポジターが通知していない物は送らない。
    fn describe(&mut self, look: Look) -> Result<(), String> {
        use wp_color_manager_v1::{Feature, RenderIntent};
        let (Some(manager), Some(representation)) =
            (&self.color_manager, &self.representation_manager)
        else {
            return Err(String::from(
                "wp_color_management_v1 と wp_color_representation_v1 の両方が要ります",
            ));
        };
        let curve = look.curve().ok_or("伝えられない曲線")?;
        let state = &self.state;
        if !state.features.contains(&(Feature::Parametric as u32)) {
            return Err(String::from(
                "伝達関数をパラメーターで記述できません（parametric が無い）",
            ));
        }
        if !state.intents.contains(&(RenderIntent::Perceptual as u32)) {
            return Err(String::from(
                "知覚的レンダリングインテント（perceptual）がありません",
            ));
        }
        if !state.curves.contains(&(curve as u32)) {
            return Err(format!("曲線 {curve:?} がありません"));
        }
        if !state.gamuts.contains(&(look.gamut() as u32)) {
            return Err(format!("色域 {:?} がありません", look.gamut()));
        }
        if !state
            .matrices
            .contains(&(look.coefficients() as u32, look.range() as u32))
        {
            return Err(format!(
                "行列と範囲 {:?}・{:?} がありません",
                look.coefficients(),
                look.range()
            ));
        }
        let params = manager.create_parametric_creator(&self.handle, ());
        params.set_tf_named(curve);
        params.set_primaries_named(look.gamut());
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
            .unwrap_or_else(|| Err(String::from("コンポジターが記述に応答しません")));
        if outcome.is_ok() {
            let surface = self
                .color_surface
                .get_or_insert_with(|| manager.get_surface(&self.surface, &self.handle, ()));
            surface.set_image_description(&description, RenderIntent::Perceptual);
            let surface = self
                .representation_surface
                .get_or_insert_with(|| representation.get_surface(&self.surface, &self.handle, ()));
            surface.set_coefficients_and_range(look.coefficients(), look.range());
        }
        description.destroy();
        outcome
    }

    pub fn resize_slots(&mut self, count: usize) {
        for slot in 0..self.slots.len() {
            self.drop_slot(slot);
        }
        self.slots = (0..count).map(|_| Vec::new()).collect();
        self.sizes = vec![None; count];
    }

    /// `fields` なら上と下のフィールドを別々に。
    pub fn hold(&mut self, slot: usize, frame: &DmaBuf, fields: bool) -> Result<(), Error> {
        self.drop_slot(slot);
        let wanted: &[Option<Parity>] = if fields {
            &[Some(Parity::Top), Some(Parity::Bottom)]
        } else {
            &[None]
        };
        let rows = if fields {
            field_rows(frame)
        } else {
            frame.height
        };
        let mut made = Vec::with_capacity(wanted.len());
        for field in wanted {
            made.push(self.buffer(frame, *field, rows)?);
        }
        if let Some(place) = self.slots.get_mut(slot) {
            *place = made;
        }
        if let Some(size) = self.sizes.get_mut(slot) {
            *size = Some((frame.width, rows));
        }
        Ok(())
    }

    pub fn holds(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(|held| !held.is_empty())
    }

    pub fn show_slot(
        &mut self,
        slot: usize,
        field: Option<Parity>,
        at: (i32, i32, i32, i32),
    ) -> Result<(), Error> {
        let held = self.slots.get(slot).map_or(&[][..], Vec::as_slice);
        let buffer = match (held, field) {
            ([one], _) => one,
            ([_, bottom], Some(Parity::Bottom)) => bottom,
            ([top, _], _) => top,
            _ => return Err(Error::Unsupported("出す wl_buffer が無い")),
        }
        .clone();
        // フィールドだけの bob では元の範囲を半行ずらす。同じ所へ伸ばすと二つのフィールドが交互に上下して
        // 震える。
        let source = match (held.len(), self.sizes.get(slot).copied().flatten()) {
            (2, Some((width, rows))) => {
                let rows = f64::from(rows);
                let from = if field == Some(Parity::Bottom) {
                    0.0
                } else {
                    0.5
                };
                Some((0.0, from, f64::from(width), rows - 0.5))
            }
            _ => None,
        };
        self.set_source(source);
        self.show(&buffer, at)
    }

    fn set_source(&mut self, source: Option<(f64, f64, f64, f64)>) {
        if self.source == source {
            return;
        }
        match source {
            Some((x, y, width, height)) => self.viewport.set_source(x, y, width, height),
            None => self.viewport.set_source(-1.0, -1.0, -1.0, -1.0),
        }
        self.source = source;
    }

    /// 付けている物はコンポジターが返したときに破棄する。
    pub fn drop_slot(&mut self, slot: usize) {
        let Some(held) = self.slots.get_mut(slot).map(std::mem::take) else {
            return;
        };
        for buffer in held {
            if self.shown.as_ref() != Some(&buffer) {
                buffer.destroy();
            }
        }
    }

    /// 記述子は呼び出し側が閉じてよい。
    pub fn buffer(
        &mut self,
        frame: &DmaBuf,
        field: Option<Parity>,
        rows: u32,
    ) -> Result<WlBuffer, Error> {
        let [luma, chroma] = frame.planes.as_slice() else {
            return Err(Error::Unsupported("輝度と色差の二つでない DMA-BUF"));
        };
        let Some(fourcc) = frame.fourcc() else {
            return Err(Error::Unsupported(
                "NV12・P010・P030・NV15 のどれでもない DMA-BUF",
            ));
        };
        let view = FieldView::of(frame, field);
        let params = self.dmabuf.create_params(&self.handle, ());
        for (index, plane) in [luma, chroma].into_iter().enumerate() {
            params.add(
                plane.fd.as_fd(),
                index as u32,
                view.offset(plane),
                view.pitch(plane),
                (frame.modifier >> 32) as u32,
                frame.modifier as u32,
            );
        }
        let buffer = params.create_immed(
            frame.width as i32,
            rows.min(view.height) as i32,
            fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &self.handle,
            self.state.released.clone(),
        );
        params.destroy();
        Ok(buffer)
    }

    /// `at` は親サーフェスでの位置（左, 上, 幅, 高さ）。
    pub fn show(&mut self, buffer: &WlBuffer, at: (i32, i32, i32, i32)) -> Result<(), Error> {
        self.dispatch();
        if self.placed != Some(at) {
            self.subsurface.set_position(at.0, at.1);
            self.viewport.set_destination(at.2.max(1), at.3.max(1));
            self.placed = Some(at);
        }
        self.surface.attach(Some(buffer), 0, 0);
        self.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        self.surface.commit();
        self.shown = Some(buffer.clone());
        self.connection
            .flush()
            .map_err(|error| Error::NoDevice(format!("Wayland: 送れません: {error}")))
    }

    pub fn dispatch(&mut self) {
        let _ = self.queue.dispatch_pending(&mut self.state);
        let released = self
            .state
            .released
            .lock()
            .map(|mut list| std::mem::take(&mut *list))
            .unwrap_or_default();
        // 付けている間に返ってきた物は外した後で破棄する（捨てると二度と返らず残る）。
        let mut shown = Vec::new();
        for buffer in released {
            if self.shown.as_ref() == Some(&buffer) {
                shown.push(buffer);
                continue;
            }
            let held = self.slots.iter().flatten().any(|slot| *slot == buffer);
            if !held {
                buffer.destroy();
            }
        }
        if !shown.is_empty()
            && let Ok(mut list) = self.state.released.lock()
        {
            list.extend(shown);
        }
    }

    /// 外した物は、スロットが持っていなければ破棄する。
    pub fn clear(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        if let Some(buffer) = self.shown.take()
            && !self.slots.iter().flatten().any(|slot| *slot == buffer)
        {
            buffer.destroy();
        }
        let _ = self.connection.flush();
    }
}

/// フレームの半分だと下のフィールドの最後の行がバッファの外にはみ出すことがあり、
/// コンポジターは接続ごと切る。
fn field_rows(frame: &DmaBuf) -> u32 {
    let half = frame.height / 2;
    let [luma, chroma] = frame.planes.as_slice() else {
        return half;
    };
    // 修飾子 0 は DRM_FORMAT_MOD_LINEAR。配置のある物は行で数えられない。
    if frame.modifier != 0 {
        return half;
    }
    let fits = |plane: &crate::decode::DmaBufPlane, per: u64| -> Option<u64> {
        let len = buffer_len(plane)?;
        let start = u64::from(plane.offset) + u64::from(plane.stride);
        let pitch = 2 * u64::from(plane.stride);
        (pitch != 0).then(|| len.saturating_sub(start) / pitch * per)
    };
    [fits(luma, 1), fits(chroma, 2)]
        .into_iter()
        .flatten()
        .fold(u64::from(half), u64::min) as u32
}

/// 記述子は末尾への移動で大きさを返す。
fn buffer_len(plane: &crate::decode::DmaBufPlane) -> Option<u64> {
    use std::io::Seek;
    let mut file = std::fs::File::from(plane.fd.try_clone().ok()?);
    file.seek(std::io::SeekFrom::End(0))
        .ok()
        .filter(|len| *len > 0)
}

impl Drop for VideoSurface {
    fn drop(&mut self) {
        for slot in 0..self.slots.len() {
            self.drop_slot(slot);
        }
        if let Some(surface) = self.color_surface.take() {
            surface.destroy();
        }
        if let Some(surface) = self.representation_surface.take() {
            surface.destroy();
        }
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        if let Some(buffer) = self.shown.take() {
            buffer.destroy();
        }
        let _ = self.connection.flush();
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, Arc<Mutex<Vec<WlBuffer>>>> for State {
    fn event(
        _: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        released: &Arc<Mutex<Vec<WlBuffer>>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event
            && let Ok(mut list) = released.lock()
        {
            list.push(buffer.clone());
        }
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_dmabuf_v1::Event::Modifier {
            format,
            modifier_hi,
            modifier_lo,
        } = event
        {
            let modifier = (u64::from(modifier_hi) << 32) | u64::from(modifier_lo);
            if !state.formats.contains(&(format, modifier)) {
                state.formats.push((format, modifier));
            }
        }
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

impl Dispatch<WpColorRepresentationManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpColorRepresentationManagerV1,
        event: wp_color_representation_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_color_representation_manager_v1::Event::SupportedCoefficientsAndRanges {
            coefficients,
            range,
        } = event
        {
            state.matrices.push((number(coefficients), number(range)));
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

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // `create_immed` の失敗は接続を切る事象で来るので、ここに来るのは `create` の物だけ。
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            tracing::warn!("Wayland: コンポジターが DMA-BUF を受け付けない");
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSubcompositor);
delegate_noop!(State: ignore WlSubsurface);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
delegate_noop!(State: ignore WpColorManagementSurfaceV1);
delegate_noop!(State: ignore WpImageDescriptionCreatorParamsV1);
delegate_noop!(State: ignore WpColorRepresentationSurfaceV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_is_told_as_the_broadcast_declares_it() {
        let bs4k = Look {
            transfer: Transfer::Hlg,
            primaries: Primaries::Bt2020,
            matrix: Matrix::Bt2020,
            limited: true,
        };
        assert!(!bs4k.plain());
        assert_eq!(
            bs4k.curve(),
            Some(wp_color_manager_v1::TransferFunction::Hlg)
        );
        assert_eq!(bs4k.gamut(), wp_color_manager_v1::Primaries::Bt2020);
        assert_eq!(
            bs4k.coefficients(),
            wp_color_representation_surface_v1::Coefficients::Bt2020
        );
        assert_eq!(
            bs4k.range(),
            wp_color_representation_surface_v1::Range::Limited
        );
        let pq = Look {
            transfer: Transfer::Pq,
            ..bs4k
        };
        assert_eq!(
            pq.curve(),
            Some(wp_color_manager_v1::TransferFunction::St2084Pq)
        );
        let hd = Look::of(&Settings::default());
        assert!(hd.plain());
        let wide_sdr = Look {
            transfer: Transfer::Sdr,
            ..bs4k
        };
        assert!(!wide_sdr.plain());
        assert_eq!(
            wide_sdr.curve(),
            Some(wp_color_manager_v1::TransferFunction::Bt1886)
        );
        assert_eq!(wp_color_manager_v1::TransferFunction::Hlg as u32, 13);
        assert_eq!(wp_color_manager_v1::Primaries::Bt2020 as u32, 6);
        assert_eq!(
            wp_color_representation_surface_v1::Coefficients::Bt2020 as u32,
            6
        );
        assert_eq!(wp_color_representation_surface_v1::Range::Limited as u32, 2);
    }
}
