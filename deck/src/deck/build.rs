// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 1 フレームをビルドする：build と、バー・ステータスバーなど上に重なる物。

use super::size::fit_rect;
use super::*;

impl Deck {
    /// 1 フレームビルドする。ウィンドウ表示ではバーが映像の外、全画面では映像の上に重なる（[`Deck::overlays`]）。
    pub fn build(&mut self, root: &mut egui::Ui, window: &Arc<Window>) {
        // 前のパスが捨てられていたなら、そこで出した要求も捨てる。
        self.passing.set(true);
        let strip_rect = self.strip_rect.take();
        self.pending.borrow_mut().clear();
        self.pending_picks.clear();
        let ctx = root.ctx().clone();
        if self.told_on_top != Some(self.on_top) {
            self.told_on_top = Some(self.on_top);
            window.set_window_level(if self.on_top {
                winit::window::WindowLevel::AlwaysOnTop
            } else {
                winit::window::WindowLevel::Normal
            });
        }
        let status_size = ui::overlay::status::text_size(self.file.ui.status_font_size);
        let arrange = ui::overlay::status::Arrange::of(&self.file.ui);
        self.status_height = ui::overlay::status::bar_height(
            &ctx,
            &self.scene.status,
            &arrange,
            status_size,
            self.file.ui.status_height,
            root.max_rect().width(),
        );
        if let Some(text) = self.copy_wanted.take() {
            ctx.copy_text(text);
        }
        self.keys(&ctx, window, strip_rect);
        self.plugin_asked(window);
        let skin = self.theme.status;
        let full = window.fullscreen().is_some();
        if self.told_full != full {
            self.told_full = full;
            self.say(ToStage::FullScreen(full));
        }
        self.full_screen = full;
        let now = Instant::now();
        let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down());
        if moved {
            self.pointer_seen = Some(now);
        }
        let whole = root.max_rect();
        let mut pressed = None;
        let mut sought: Option<ui::overlay::transport::Pressed> = None;
        let area = if full {
            self.bar = 0.0;
            whole
        } else {
            if self.menus.in_window() && self.file.ui.menu_bar {
                let menus = &self.menus;
                let filling = self.filling();
                let bar = egui::Panel::top("メニュー")
                    .frame(
                        egui::Frame::NONE
                            .fill(skin.background)
                            .inner_margin(egui::Margin::symmetric(4, 2)),
                    )
                    .show_separator_line(false)
                    .show(root, |ui| menus.show(ui, &filling));
                self.bar = bar.response.rect.height();
                self.shown_on_bar(bar.inner, window);
            } else {
                self.bar = 0.0;
            }
            if self.file.ui.status_bar {
                let clicked = egui::Panel::bottom("ステータスバー")
                    .exact_size(self.status_height)
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::NONE
                            .fill(skin.background)
                            .stroke(egui::Stroke::new(1.0, skin.edge)),
                    )
                    .show(root, |painting| {
                        ui::overlay::status::show(
                            painting,
                            &self.scene.status,
                            &skin,
                            status_size,
                            &arrange,
                        )
                    })
                    .inner;
                self.status_clicked(clicked);
            }
            if let Some(progress) = self.scene.progress {
                let seek = &mut self.seek;
                sought = egui::Panel::bottom("録画の操作")
                    .exact_size(ui::theme::transport::HEIGHT)
                    .show_separator_line(false)
                    .frame(
                        egui::Frame::NONE
                            .fill(skin.background)
                            .stroke(egui::Stroke::new(1.0, skin.edge)),
                    )
                    .show(root, |painting| {
                        ui::overlay::transport::show(painting, &progress, &skin, seek)
                    })
                    .inner;
            }
            let panel = self.panel_height();
            if panel > 0.0 {
                let shown = egui::Panel::bottom("選局番組表")
                    .exact_size(panel)
                    .show_separator_line(false)
                    .frame(egui::Frame::NONE)
                    .show(root, |painting| self.show_panel(painting, whole));
                pressed = shown.inner;
                self.strip_rect = Some(shown.response.rect);
            }
            root.available_rect_before_wrap()
        };
        self.backdrop(root, area);
        let grabbed = root.interact(
            area,
            root.scope_id().with("映像"),
            egui::Sense::click_and_drag(),
        );
        // クリックの動作は設定で決める。メニューは出せる環境では OS のもの、出せなければ egui で。
        let filling = self.filling();
        let clicked: Vec<Click> = [
            (grabbed.double_clicked(), &self.file.operation.double_click),
            (grabbed.middle_clicked(), &self.file.operation.middle_click),
            (
                grabbed.secondary_clicked(),
                &self.file.operation.right_click,
            ),
        ]
        .into_iter()
        .filter(|(clicked, _)| *clicked)
        .map(|(_, named)| Click::parse(named))
        .collect();
        let mut menu_click = false;
        for click in clicked {
            match click {
                Click::Nothing => {}
                Click::Menu => menu_click = true,
                Click::Act(action) => self.act_on(action, window),
            }
        }
        if menu::Menus::has_native_context() {
            self.context_wanted |= menu_click;
        } else {
            let opening = if menu_click {
                Some(egui::SetOpenCommand::Bool(true))
            } else if grabbed.clicked() {
                Some(egui::SetOpenCommand::Bool(false))
            } else {
                None
            };
            let mut picked = None;
            egui::Popup::menu(&grabbed)
                .open_memory(opening)
                .at_pointer_fixed()
                .show(|ui| picked = menu::egui::context(ui, &filling));
            if let Some(picked) = picked {
                self.pick(picked, window);
            }
        }
        if grabbed.drag_started() && !full {
            let _ = window.drag_window();
        }
        // データ放送。文書が出ている間は文書の矩形が主で、映像・OSD・字幕はそれに合わせる。
        // レイヤーの重なりは映像 → データ放送 → 字幕 → OSD（`zwsdk::layer::Place`）。
        use zwsdk::layer::Slot;
        let layers = self.take_layers();
        let names = crate::capture::plugin_rows(&self.scene.video_filters);
        if self.told_layers.as_ref() != Some(&names) {
            let _ = self.to_panes.send(ToPanes::CaptureLayers(names.clone()));
            self.told_layers = Some(names);
        }
        self.show_layers(
            root,
            area,
            &layers,
            (Slot::Video, 0),
            (Slot::Databroadcast, 0),
        );
        let frame = self.databroadcast(root, area);
        self.tell_area(&ctx, frame);
        self.show_layers(
            root,
            frame,
            &layers,
            (Slot::Databroadcast, 2),
            (Slot::Captions, 1),
        );
        let captions = if Self::replaced(&layers, Slot::Captions) {
            None
        } else {
            self.shown_captions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        };
        if let Some(captions) = captions {
            // 字幕は切り取る前の映像全体に合わせて置く。
            let video = self.scene.uncropped(frame);
            let mut moving = false;
            let caption_decor = ui::overlay::caption::Decor::of(&self.file.caption, false);
            let superimpose_decor = ui::overlay::caption::Decor::of(&self.file.caption, true);
            // 出すかどうかは data のスレッドが決めている（[`zwcore::captions::Choice::actual`]）。
            let arib = captions
                .caption
                .iter()
                .map(|shown| (shown, &caption_decor))
                .chain(
                    captions
                        .superimpose
                        .iter()
                        .map(|shown| (shown, &superimpose_decor)),
                );
            for ((layout, since), decor) in arib {
                ui::overlay::caption::show(root, video, layout, since.elapsed(), decor);
                moving |= ui::overlay::caption::animates(layout);
            }
            let ttml = captions
                .ttml_caption
                .iter()
                .map(|shown| (shown, &caption_decor))
                .chain(
                    captions
                        .ttml_superimpose
                        .iter()
                        .map(|shown| (shown, &superimpose_decor)),
                );
            for (shown, decor) in ttml {
                ui::overlay::ttml::show(root, video, shown, decor);
                moving |= shown.animates();
            }
            if moving {
                root.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(40));
            }
        }
        self.show_layers(root, frame, &layers, (Slot::Captions, 1), (Slot::Osd, 1));
        // 映像の無い番組では 16:9 の枠を想定する（でないとウィンドウいっぱいになる）。
        let osd_frame = if self.scene.ratio.is_none() && frame == area {
            fit_rect(area, 16.0 / 9.0, root.ctx().pixels_per_point())
        } else {
            frame
        };
        if let Some(idle) = &self.scene.idle
            && !self.page_shown()
        {
            ui::overlay::idle::show(root, osd_frame, idle);
        }
        if let Some(trouble) = &self.scene.trouble {
            ui::overlay::trouble::show(root, area, osd_frame, trouble);
        }
        if let Some((osd, opacity)) = &self.scene.osd
            && self.file.osd.enabled
            && !Self::replaced(&layers, Slot::Osd)
        {
            let shade = self.file.osd.opacity.min(100) as f32 / 100.0;
            ui::overlay::osd::show(root, osd_frame, osd, *opacity * shade, self.scene.hdr);
        }
        self.show_layers(root, osd_frame, &layers, (Slot::Osd, 1), (Slot::Osd, 3));
        if let Some(name) = &self.scene.status.emergency {
            ui::overlay::caption::emergency(root, frame, name);
        }
        let mut toast_frame = frame;
        if full {
            let covered = self.overlays(root, window, whole, &skin, &mut pressed, &mut sought);
            toast_frame.max.y = toast_frame.max.y.min(whole.bottom() - covered);
        } else {
            self.show_cursor(window, true);
        }
        if let Some(pressed) = sought {
            self.transport(pressed);
        }
        if self.seek.dragging.is_some() {
            root.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        if !menu::Menus::has_native_context()
            && let Some(items) = self.status_menu
        {
            let at = *self.status_menu_at.get_or_insert_with(|| {
                ctx.pointer_latest_pos().unwrap_or(whole.center()) - egui::vec2(0.0, 220.0)
            });
            let filling = self.filling();
            let mut picked = None;
            let shown = egui::Area::new(egui::Id::unique("ステータスバーのメニュー"))
                .order(egui::Order::Foreground)
                .fixed_pos(at)
                .show(&ctx, |ui| {
                    egui::Frame::menu(ui.style()).show(ui, |ui| {
                        picked = menu::egui::items_of(ui, items, &filling);
                    });
                });
            let outside =
                ctx.input(|i| i.pointer.any_pressed()) && !shown.response.contains_pointer();
            if let Some(picked) = picked {
                self.status_menu = None;
                self.pick(picked, window);
            } else if outside {
                self.status_menu = None;
            }
        }
        if let Some((volume, opacity)) = &self.scene.volume
            && self.file.osd.enabled
        {
            let shade = self.file.osd.opacity.min(100) as f32 / 100.0;
            let covered = (osd_frame.bottom() - toast_frame.bottom()).max(0.0);
            ui::overlay::volume::show(
                root,
                osd_frame,
                volume,
                *opacity * shade,
                covered,
                self.scene.hdr,
            );
            if *opacity < 1.0 {
                root.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if let Some(page) = self.settings_at_start.take() {
            let _ = self.to_panes.send(ToPanes::OpenSettings(page));
        }
        if std::mem::take(&mut self.favourites_at_start) {
            let _ = self
                .to_panes
                .send(ToPanes::Open(window::sub::Which::Favourites));
        }
        if std::mem::take(&mut self.about_at_start) {
            let _ = self.to_panes.send(ToPanes::Open(window::sub::Which::About));
        }
        if let Some(&at) = self.capture_at.first() {
            if std::time::Instant::now() >= at {
                self.capture_at.remove(0);
                self.capture_wanted = Some(false);
            }
            root.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        if let Some(copy) = self.capture_wanted.take() {
            self.send_capture(copy, &layers, frame, osd_frame, ctx.pixels_per_point());
        }
        if let Some((toast, opacity)) = &self.scene.toast {
            ui::overlay::osd::toast(root, toast_frame, toast, *opacity, self.scene.hdr);
            root.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(pressed) = pressed {
            self.panel_pressed(pressed);
        }
        self.tell_rows();
        self.save_panel(&ctx);
        self.fit_window(window);
        self.settle_ratio(&ctx, window);
    }

    /// 全画面で映像の上に重ねるバー。返すのは下端で覆った高さ。
    fn overlays(
        &mut self,
        root: &mut egui::Ui,
        window: &Arc<Window>,
        whole: egui::Rect,
        skin: &ui::theme::status::Skin,
        pressed: &mut Option<ui::guide::panel::Pressed>,
        sought: &mut Option<ui::overlay::transport::Pressed>,
    ) -> f32 {
        use ui::theme::overlay::HOLD;
        let ctx = root.ctx().clone();
        let now = Instant::now();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let strip_height = self.panel_height();
        let transport_height = if self.scene.progress.is_some() {
            ui::theme::transport::HEIGHT
        } else {
            0.0
        };
        let bottom_height = self.status_height + transport_height + strip_height;
        let menu_height = if self.menus.in_window() {
            ui::theme::overlay::MENU_HEIGHT
        } else {
            0.0
        };
        let at_bottom = pointer.is_some_and(|p| p.y >= whole.bottom() - bottom_height);
        let at_top = menu_height > 0.0 && pointer.is_some_and(|p| p.y <= whole.top() + menu_height);
        if at_bottom || self.seek.dragging.is_some() {
            self.bottom_until = Some(now + HOLD);
        }
        // 開いているポップアップは egui のメモリーで見る。`Context::any_popup_open` はバーを表示する前のここでは常に空。
        if at_top || egui::Popup::is_any_open(&ctx) {
            self.top_until = Some(now + HOLD);
        }
        let show_bottom = self.bottom_until.is_some_and(|until| now < until);
        let show_top = self.top_until.is_some_and(|until| now < until);
        let mut covered = 0.0;
        if show_bottom {
            let band = egui::Rect::from_min_max(
                egui::pos2(whole.left(), whole.bottom() - self.status_height),
                whole.max,
            );
            let mut painting = root.new_child(egui::UiBuilder::new().max_rect(band));
            painting.painter().rect_filled(band, 0.0, skin.background);
            painting.painter().hline(
                band.x_range(),
                band.top(),
                egui::Stroke::new(1.0, skin.edge),
            );
            let clicked = ui::overlay::status::show(
                &mut painting,
                &self.scene.status,
                skin,
                ui::overlay::status::text_size(self.file.ui.status_font_size),
                &ui::overlay::status::Arrange::of(&self.file.ui),
            );
            self.status_clicked(clicked);
            covered = self.status_height;
            let mut top = band.top();
            if let Some(progress) = self.scene.progress {
                let row = egui::Rect::from_min_max(
                    egui::pos2(whole.left(), top - ui::theme::transport::HEIGHT),
                    egui::pos2(whole.right(), top),
                );
                let mut painting = root.new_child(egui::UiBuilder::new().max_rect(row));
                painting.painter().rect_filled(row, 0.0, skin.background);
                painting.painter().hline(
                    row.x_range(),
                    row.top(),
                    egui::Stroke::new(1.0, skin.edge),
                );
                *sought =
                    ui::overlay::transport::show(&mut painting, &progress, skin, &mut self.seek);
                covered += ui::theme::transport::HEIGHT;
                top = row.top();
            }
            if strip_height > 0.0 {
                let above = egui::Rect::from_min_max(
                    egui::pos2(whole.left(), top - strip_height),
                    egui::pos2(whole.right(), top),
                );
                let mut painting = root.new_child(egui::UiBuilder::new().max_rect(above));
                *pressed = self.show_panel(&mut painting, whole);
                self.strip_rect = Some(above);
                covered += strip_height;
            }
        }
        if show_top && menu_height > 0.0 {
            let band = egui::Rect::from_min_max(
                whole.min,
                egui::pos2(whole.right(), whole.top() + menu_height),
            );
            let mut painting = root.new_child(egui::UiBuilder::new().max_rect(band));
            let menus = &self.menus;
            let filling = self.filling();
            let picked = egui::Frame::NONE
                .fill(skin.background)
                .inner_margin(egui::Margin::symmetric(4, 2))
                .show(&mut painting, |ui| {
                    ui.set_min_width(band.width());
                    menus.show(ui, &filling)
                })
                .inner;
            self.shown_on_bar(picked, window);
        }
        if show_bottom || show_top {
            let until = [self.bottom_until, self.top_until]
                .into_iter()
                .flatten()
                .max()
                .unwrap_or(now);
            ctx.request_repaint_after(
                until.saturating_duration_since(now) + Duration::from_millis(16),
            );
        }
        // 0 秒なら隠さない。
        let idle_cursor = Duration::from_secs(u64::from(self.file.general.hide_cursor));
        let idle = !idle_cursor.is_zero()
            && self
                .pointer_seen
                .is_none_or(|seen| now.duration_since(seen) >= idle_cursor);
        let hide = idle && !show_bottom && !show_top && !self.menu_open;
        self.show_cursor(window, !hide);
        if !hide
            && !idle_cursor.is_zero()
            && let Some(seen) = self.pointer_seen
        {
            ctx.request_repaint_after((seen + idle_cursor).saturating_duration_since(now));
        }
        covered
    }

    pub(super) fn status_clicked(&mut self, clicked: Option<ui::overlay::status::Clicked>) {
        match clicked {
            Some(ui::overlay::status::Clicked::Channel) => self.set_mini(!self.mini),
            Some(ui::overlay::status::Clicked::Sound) => self.say(ToStage::Act(Act::NextSound)),
            Some(ui::overlay::status::Clicked::Volume(volume)) => {
                self.say(ToStage::Act(Act::SetVolume(volume)));
            }
            Some(ui::overlay::status::Clicked::Mute) => self.say(ToStage::Act(Act::ToggleMute)),
            Some(ui::overlay::status::Clicked::Record) => {
                self.say(ToStage::Act(Act::Record { status_bar: true }));
            }
            Some(ui::overlay::status::Clicked::Capture) => {
                self.capture_wanted = Some(!self.file.capture.icon_saves);
            }
            Some(ui::overlay::status::Clicked::RecordMenu) => {
                self.status_menu = Some(menu::RECORD_MENU);
                self.status_menu_at = None;
            }
            Some(ui::overlay::status::Clicked::CaptureMenu) => {
                self.status_menu = Some(menu::CAPTURE_MENU);
                self.status_menu_at = None;
            }
            Some(ui::overlay::status::Clicked::ErrorsMenu) => {
                self.status_menu = Some(menu::ERRORS_MENU);
                self.status_menu_at = None;
            }
            None => {}
        }
    }

    fn transport(&mut self, pressed: ui::overlay::transport::Pressed) {
        use ui::overlay::transport::Pressed;
        let act = match pressed {
            Pressed::Seek(at) => Act::Seek(at),
            Pressed::SeekBy(ms) => Act::SeekBy(ms),
            Pressed::Pause => Act::Pause,
            Pressed::Pace(pace) => Act::Pace(pace),
        };
        self.say(ToStage::Act(act));
    }

    pub(super) fn show_bottom(&mut self) {
        self.bottom_until = Some(Instant::now() + ui::theme::overlay::AFTER_KEY);
    }

    pub fn menu_showing(&mut self, open: bool) {
        self.menu_open = open;
        if !open {
            self.pointer_seen = Some(Instant::now());
        }
    }

    fn show_cursor(&mut self, window: &Window, shown: bool) {
        if self.cursor_hidden == !shown {
            return;
        }
        self.cursor_hidden = !shown;
        window.set_cursor_visible(shown);
    }

    pub fn broadcast_changed(&self) -> bool {
        let now = self
            .shown_broadcast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|shown| Arc::as_ptr(shown) as usize);
        now != self.drawn_broadcast
    }

    pub fn layers_changed(&self) -> bool {
        let now =
            Arc::as_ptr(&self.shown_layers.lock().unwrap_or_else(|e| e.into_inner())) as usize;
        Some(now) != self.drawn_layers
    }
}

/// 映像の上のクリック（`[operation]` の `double_click`・`middle_click`・`right_click`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Click {
    Nothing,
    Menu,
    Act(crate::keys::Action),
}

impl Click {
    /// `nothing`・`menu`、ほかはキー割り当てと同じコマンドの名前。読めなければ何もしない。
    pub(crate) fn parse(name: &str) -> Self {
        match name.trim() {
            "menu" => Self::Menu,
            "nothing" | "" => Self::Nothing,
            other => crate::keys::Action::parse(other).map_or(Self::Nothing, Self::Act),
        }
    }
}

#[cfg(test)]
mod click_tests {
    use super::Click;
    use crate::keys::Action;
    use crate::window::menu::Command;

    #[test]
    fn a_click_names_a_command_like_a_key() {
        assert_eq!(Click::parse("menu"), Click::Menu);
        assert_eq!(Click::parse("nothing"), Click::Nothing);
        assert_eq!(Click::parse("mute"), Click::Act(Action::Mute));
        assert_eq!(
            Click::parse("fullscreen"),
            Click::Act(Action::Menu(Command::FullScreen))
        );
        assert_eq!(Click::parse("知らない"), Click::Nothing);
    }
}
