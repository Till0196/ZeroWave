// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! データ放送・字幕などのレイヤーと静止画の保存、背景。

use super::size::fit_rect;
use super::*;

impl Deck {
    pub(super) fn databroadcast(&mut self, root: &mut egui::Ui, area: egui::Rect) -> egui::Rect {
        let shown = self
            .shown_broadcast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        // C プロファイルの文書は映像に重ねず別ウィンドウへ。
        let one_seg = shown.as_ref().is_some_and(|shown| shown.c_profile);
        if one_seg != self.one_seg_popup {
            self.one_seg_popup = one_seg;
            let _ = self.to_panes.send(if one_seg {
                ToPanes::Open(Which::OneSeg)
            } else {
                ToPanes::ShutOneSeg
            });
        }
        let shown = shown.filter(|shown| !shown.c_profile);
        let ppp = root.ctx().pixels_per_point();
        let page_ratio = shown
            .as_ref()
            .filter(|shown| !shown.invisible && self.broadcast_on)
            .filter(|shown| shown.size.0 > 0.0 && shown.size.1 > 0.0)
            .map(|shown| self.page_ratio(shown));
        let video = match page_ratio {
            Some(ratio) => fit_rect(area, ratio, ppp),
            None => self.video_rect(area, ppp),
        };
        let area = (
            (video.width() * ppp).round() as u32,
            (video.height() * ppp).round() as u32,
        );
        if self.told_page_area != Some(area) && area.0 > 0 && area.1 > 0 {
            self.told_page_area = Some(area);
            let _ = self
                .to_broadcast
                .try_send(ToBroadcast::Area(area.0, area.1));
        }
        self.drawn_broadcast = shown.as_ref().map(|shown| Arc::as_ptr(shown) as usize);
        let inset = shown
            .as_ref()
            .filter(|shown| !shown.invisible && self.broadcast_on)
            .and_then(|shown| {
                let rect = shown.video?;
                let (w, h) = shown.size;
                if w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let whole = rect.x == 0.0 && rect.y == 0.0 && rect.w >= w && rect.h >= h;
                (!whole).then(|| {
                    [
                        rect.x / w,
                        rect.y / h,
                        rect.w / w,
                        rect.h / h,
                        self.page_ratio(shown),
                    ]
                })
            });
        let key = inset.map(|i| i.map(|v| (v * 1000.0).round() as u32));
        if self.told_inset != Some(key) {
            self.told_inset = Some(key);
            self.say(ToStage::Inset(inset));
        }
        let ready = one_seg
            || shown
                .as_ref()
                .is_some_and(|shown| !shown.invisible && !shown.waiting);
        let channel = self.scene.status.channel.clone();
        if let Some((_, asked_on @ None)) = &mut self.data_asked {
            asked_on.clone_from(&channel);
        }
        let retuned = |asked_on: &Option<String>| {
            asked_on.is_some() && channel.is_some() && *asked_on != channel
        };
        let absent = self.scene.status.data == Some(false);
        if ready
            || absent
            || self
                .data_asked
                .as_ref()
                .is_some_and(|(at, asked_on)| at.elapsed() > DATA_ASKED_FOR || retuned(asked_on))
        {
            self.data_asked = None;
        }
        if let Some(shown) = &shown
            && self.broadcast_on
        {
            self.broadcast_textures.keep_only(shown);
            ui::panes::databroadcast::show(root, video, shown, &mut self.broadcast_textures);
        }
        if self.broadcast_on && self.data_asked.is_some() && !video.is_negative() {
            let size = (15.0 * video.height() / 540.0).clamp(11.0, 20.0);
            ui::panes::databroadcast::loading(root, video, size);
        }
        video
    }

    pub(super) fn asked_for_data(&mut self) {
        let one_seg = self
            .shown_broadcast
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|shown| shown.c_profile);
        if one_seg {
            return;
        }
        if self.scene.status.data == Some(false) {
            self.data_asked = None;
            return;
        }
        self.data_asked = Some((Instant::now(), self.scene.status.channel.clone()));
    }

    pub(super) fn take_layers(&mut self) -> Arc<Vec<Arc<zwcore::layers::Layer>>> {
        let layers = Arc::clone(&self.shown_layers.lock().unwrap_or_else(|e| e.into_inner()));
        self.drawn_layers = Some(Arc::as_ptr(&layers) as usize);
        let names: std::collections::BTreeSet<&str> =
            layers.iter().map(|layer| layer.name.as_str()).collect();
        self.layer_textures
            .retain(|name, _| names.contains(name.as_str()));
        layers
    }

    /// 画像キャプチャをビルドして再生スレッドへ渡す。`osd_frame` は OSD を置く矩形。
    pub(super) fn send_capture(
        &mut self,
        copy: bool,
        layers: &[Arc<zwcore::layers::Layer>],
        frame: egui::Rect,
        osd_frame: egui::Rect,
        ppp: f32,
    ) {
        let target = if self.scene.ratio.is_none() {
            osd_frame
        } else {
            frame
        };
        if target.width() < 1.0 || target.height() < 1.0 {
            self.say(ToStage::Act(Act::Toast(String::from(
                "キャプチャできる映像がありません。",
            ))));
            return;
        }
        let ratio = target.width() / target.height();
        let display = (
            (target.width() * ppp).round() as u32,
            (target.height() * ppp).round() as u32,
        );
        // 映像と同じ縦横比（1% 以内）なら映像の幅のまま。
        let source = self.scene.picture.map(|(width, height)| {
            let own = width as f32 / height.max(1) as f32;
            if (own / ratio - 1.0).abs() < 0.01 {
                (width, height)
            } else {
                (((height as f32) * ratio).round() as u32, height)
            }
        });
        let size = crate::capture::size_of(&self.file.capture.size, source, display);
        let page = self.page_shown().then(|| {
            self.shown_broadcast
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        });
        let captions = if Self::replaced(layers, zwsdk::layer::Slot::Captions) {
            None
        } else {
            self.shown_captions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        };
        let shade = self.file.osd.opacity.min(100) as f32 / 100.0;
        let osd = self
            .scene
            .osd
            .as_ref()
            .filter(|_| self.file.osd.enabled)
            .map(|(osd, opacity)| (osd, *opacity * shade));
        let volume = self
            .scene
            .volume
            .as_ref()
            .filter(|_| self.file.osd.enabled)
            .map(|(volume, opacity)| (volume, *opacity * shade));
        let idle = self.scene.idle.as_deref().filter(|_| !self.page_shown());
        let composer = self
            .composer
            .get_or_insert_with(crate::capture::Composer::new);
        let (drawn, video) = composer.compose(&crate::capture::Shown {
            size,
            ratio,
            layers,
            broadcast: page.flatten(),
            captions,
            osd,
            volume,
            idle,
            hdr: self.scene.hdr,
            file: &self.file,
        });
        let channel = self.scene.status.channel.clone().unwrap_or_default();
        let event = ui::text::badges::bracketed(&self.scene.event);
        let name = zwcore::record::file_name(
            &self.file.capture.name,
            &channel,
            &event,
            self.file.guide.offset,
        );
        let folder =
            crate::capture::folder_of(&self.file.capture.folder, crate::capture::Folder::Pictures);
        let format = self.file.capture.format.clone();
        let hdr = self.file.capture.hdr && !copy && crate::capture::extension(&format) == "png";
        let path = folder.join(format!("{name}.{}", crate::capture::extension(&format)));
        let job = crate::capture::Job {
            drawn,
            size,
            video: (
                video.left().max(0.0) as u32,
                video.top().max(0.0) as u32,
                video.width() as u32,
                video.height() as u32,
            ),
            with_video: self.scene.backdrop == Backdrop::None,
            filtered: self.scene.video_filters.is_empty()
                || self.file.capture.includes(crate::capture::VIDEO_FILTER),
            copy,
            path,
            encoding: crate::capture::Encoding {
                format,
                jpeg_quality: self.file.capture.jpeg_quality,
                png_level: self.file.capture.png_level,
            },
            hdr,
        };
        self.say(ToStage::Capture(Box::new(job)));
    }

    pub(super) fn page_shown(&self) -> bool {
        self.broadcast_on
            && self
                .shown_broadcast
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(|shown| !shown.invisible && !shown.c_profile)
    }

    pub(super) fn replaced(
        layers: &[Arc<zwcore::layers::Layer>],
        slot: zwsdk::layer::Slot,
    ) -> bool {
        layers
            .iter()
            .any(|layer| layer.place == zwsdk::layer::Place::At(slot))
    }

    pub(super) fn show_layers(
        &mut self,
        root: &mut egui::Ui,
        video: egui::Rect,
        layers: &[Arc<zwcore::layers::Layer>],
        from: (zwsdk::layer::Slot, u8),
        to: (zwsdk::layer::Slot, u8),
    ) {
        let ctx = root.ctx().clone();
        let painter = root.painter().with_clip_rect(video);
        for layer in layers
            .iter()
            .filter(|layer| (from..to).contains(&layer.place.order()))
        {
            let key = Arc::as_ptr(&layer.image) as usize;
            let handle = match self.layer_textures.get_mut(&layer.name) {
                Some((had, handle)) if *had == key => handle.clone(),
                Some((had, handle)) => {
                    handle.set(
                        crate::ui::rgba_image(&layer.image),
                        egui::TextureOptions::LINEAR,
                    );
                    *had = key;
                    handle.clone()
                }
                None => {
                    let handle = ctx.load_texture(
                        format!("レイヤー {}", layer.name),
                        crate::ui::rgba_image(&layer.image),
                        egui::TextureOptions::LINEAR,
                    );
                    self.layer_textures
                        .insert(layer.name.clone(), (key, handle.clone()));
                    handle
                }
            };
            painter.image(
                handle.id(),
                video,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }

    pub(super) fn backdrop(&self, root: &mut egui::Ui, area: egui::Rect) {
        match self.scene.backdrop {
            Backdrop::None => {}
            Backdrop::Waiting => {
                let mut painting = root.new_child(egui::UiBuilder::new().max_rect(area));
                ui::overlay::status::empty_scene(&mut painting, area);
            }
            // 黒は再生スレッドが塗る（ここでレンダリングするとメニューのモーダルループの間、前の映像の上に残る）。
            Backdrop::Black => {}
        }
    }
}
