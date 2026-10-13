// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画像キャプチャの再生スレッド側。最後に出した映像をレンダリングし直し、図形を重ね、読み戻す。

use super::*;

mod convert;

pub(super) use convert::Converter;

pub(super) struct Pending {
    readback: crate::capture::Readback,
    state: Arc<std::sync::atomic::AtomicU8>,
    copy: bool,
    path: std::path::PathBuf,
    encoding: crate::capture::Encoding,
    started: Instant,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Pending")
            .field("size", &self.readback.size)
            .finish_non_exhaustive()
    }
}

pub(super) type Warm = (wgpu::TextureFormat, Space, overlay::Mixer, Converter);

/// GLES 2.0 のバックエンドで、1 フレームの間に読み戻しへ使ってよい時間（映像の処理を邪魔しない程度）。
#[cfg(all(unix, not(target_os = "macos")))]
const READ_EACH_TURN: Duration = Duration::from_millis(4);

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) struct Reading {
    readback: zwcore::output::present::Readback,
    copy: bool,
    path: std::path::PathBuf,
    encoding: crate::capture::Encoding,
    started: Instant,
}

const CAPTURES_AT_ONCE: usize = 2;

/// キャプチャのレンダラーをバックグラウンドでビルドし始める（150 ms ほどかかる）。
pub(super) fn warm(
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    space: Space,
    white: f32,
) -> Option<Receiver<Warm>> {
    let (to, from) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(String::from("capture-warm"))
        .spawn(move || {
            let mixer = overlay::Mixer::new(&device, format, space, white);
            let converter = Converter::new(&device);
            let _ = to.send((format, space, mixer, converter));
        });
    spawned.is_ok().then_some(from)
}

impl Stage {
    pub(super) fn capture(&mut self, mut job: Box<crate::capture::Job>) {
        use crate::capture::Layout;
        use convert::Mode;
        let started = Instant::now();
        // 前に見送った更新の文字の差分をまとめて反映する（捨てるとテクスチャとずれ、文字が欠ける）。
        if let Some(mut older) = self.capture_carry.take() {
            job.drawn.fold_into(&mut older);
        }
        // 同時に抱えるのは 2 フレームまで（8K だと 1 フレームで読み戻し 130 MB）。
        #[cfg(all(unix, not(target_os = "macos")))]
        let reading = self.capture_reading.len();
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let reading = 0;
        if self.capture_pending.len() + reading + crate::capture::writing() >= CAPTURES_AT_ONCE {
            self.toast("前回のキャプチャを保存しています。\nしばらくお待ちください。");
            self.capture_carry = Some(job.drawn);
            return;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if matches!(self.painter, Some(Painter::Gles { .. })) {
            self.capture_gles(job, started);
            return;
        }
        let Some(Painter::Wgpu { gpu, .. }) = self.painter.as_ref() else {
            self.toast("このレンダリング方式ではキャプチャを利用できません。");
            self.capture_carry = Some(job.drawn);
            return;
        };
        let device = gpu.device.clone();
        let queue = gpu.queue.clone();
        let format = self.surface.target;
        let space = self.surface.space;
        let white = self.surface.sdr_white;
        let (width, height) = job.size;
        if width == 0 || height == 0 {
            self.capture_carry = Some(job.drawn);
            return;
        }
        // バックグラウンドでビルドしたレンダラーは、まだ持っていないときだけ使う。
        if let Some(warm) = self.capture_warm.take()
            && let Ok((had, was, mixer, converter)) = warm.recv()
            && self.capture_mixer.is_none()
        {
            self.capture_mixer = Some((had, was, mixer));
            self.capture_convert = Some(converter);
        }
        let plain = format.remove_srgb_suffix();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("キャプチャ"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: if format.is_srgb() {
                std::slice::from_ref(&plain)
            } else {
                &[]
            },
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        blackout(&device, &queue, &view);
        if job.with_video {
            let (left, top, w, h) = job.video;
            let onto = Onto {
                left,
                top,
                width: w.max(1),
                height: h.max(1),
            };
            let canvas = Canvas::Wgpu { view: &view, onto };
            if job.filtered {
                self.player.redraw(&canvas);
            } else {
                self.player.redraw_unfiltered(&canvas);
            }
        }
        let drawn_video = started.elapsed();
        // 形式か色空間が変わったら出力先のテクスチャだけを替える（レンダラーを作り直すと文字が欠ける）。
        match self.capture_mixer.as_mut() {
            Some((had, was, mixer)) => {
                if *had != format || *was != space {
                    mixer.retarget(&device, format, space, white);
                    *had = format;
                    *was = space;
                }
            }
            None => {
                self.capture_mixer = Some((
                    format,
                    space,
                    overlay::Mixer::new(&device, format, space, white),
                ));
            }
        }
        let Job {
            mut drawn,
            copy,
            path,
            encoding,
            hdr,
            ..
        } = *job;
        if let Some((_, _, mixer)) = self.capture_mixer.as_mut() {
            mixer.draw(&device, &queue, &texture, (width, height), &mut drawn);
        }
        // HDR のまま書くのは、設定で指定されていて画面が HDR のときだけ。scRGB は GPU で PQ に直す。
        let keep_hdr = hdr && space != Space::Srgb;
        let (layout, mode) = match (keep_hdr, format, space) {
            (true, wgpu::TextureFormat::Rgb10a2Unorm, Space::Pq) => {
                (Layout::Rgb10a2 { transfer: 16 }, None)
            }
            (true, wgpu::TextureFormat::Rgb10a2Unorm, Space::Hlg) => {
                (Layout::Rgb10a2 { transfer: 18 }, None)
            }
            (true, wgpu::TextureFormat::Rgba16Float, _) => (Layout::Half, Some(Mode::ScRgbPq)),
            _ => (Layout::Rgba8, Some(Mode::to_sdr(format, space))),
        };
        // scRGB の 1：Windows と Vulkan は 80 cd/m²、Apple は SDR の白。
        let one = match mode {
            Some(Mode::ScRgbPq) if cfg!(target_os = "macos") => white.max(1.0),
            Some(Mode::ScRgbPq) => 80.0,
            Some(Mode::ScRgbSdr) if cfg!(target_os = "macos") => 1.0,
            Some(Mode::ScRgbSdr) => 80.0 / white.max(1.0),
            _ => 1.0,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("キャプチャの読み戻し"),
        });
        let converted = mode.map(|mode| {
            let target_format = if layout == Layout::Half {
                convert::HALF
            } else {
                convert::SDR
            };
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("キャプチャの色変換"),
                size: texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let converter = self
                .capture_convert
                .get_or_insert_with(|| Converter::new(&device));
            converter.run(
                &device,
                &queue,
                &mut encoder,
                &view,
                &target_view,
                target_format,
                mode,
                white,
                one,
            );
            target
        });
        let source = converted.as_ref().unwrap_or(&texture);
        let Some(bytes) = source.format().block_copy_size(None) else {
            self.toast("キャプチャを利用できません。");
            return;
        };
        let row = width * bytes;
        let stride =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("キャプチャの読み戻し"),
            size: u64::from(stride * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            source.size(),
        );
        queue.submit([encoder.finish()]);
        let state = Arc::new(std::sync::atomic::AtomicU8::new(0));
        {
            let state = Arc::clone(&state);
            buffer.map_async(wgpu::MapMode::Read, .., move |answer| {
                let value = if answer.is_ok() { 1 } else { 2 };
                state.store(value, std::sync::atomic::Ordering::Release);
            });
        }
        self.capture_pending.push(Pending {
            readback: crate::capture::Readback {
                source: crate::capture::Source::Gpu(buffer),
                stride,
                size: (width, height),
                layout,
            },
            state,
            copy,
            path,
            encoding,
            started,
        });
        tracing::info!(
            "キャプチャ {width}x{height} {format:?} {space:?}：映像のレンダリング {drawn_video:?}、再生スレッドの所要時間 {:?}",
            started.elapsed()
        );
    }

    /// GLES 2.0 のバックエンドのキャプチャ。その場で読み戻す（ES 2.0 には待たずに読む方法が無い）。HDR は無い。
    #[cfg(all(unix, not(target_os = "macos")))]
    fn capture_gles(&mut self, job: Box<crate::capture::Job>, started: Instant) {
        let Job {
            drawn,
            size,
            video,
            with_video,
            copy,
            path,
            encoding,
            ..
        } = *job;
        if size.0 == 0 || size.1 == 0 {
            self.capture_carry = Some(drawn);
            return;
        }
        self.capture_layer.hand(drawn);
        let layer = &self.capture_layer;
        let video = with_video.then_some(video);
        let taken = match self.player.capture_gles(size, video, Some(layer)) {
            Some(taken) => taken,
            None => self
                .painter
                .as_ref()
                .ok_or(zwcore::output::video::Error::Unsupported(
                    "レンダラーが無い",
                ))
                .and_then(|painter| painter.gles_window())
                .and_then(|window| window.capture(size, &mut |_, _| {}, Some(layer))),
        };
        let readback = match taken {
            Ok(readback) => readback,
            Err(error) => {
                tracing::warn!("キャプチャ：{error:?}");
                self.toast("キャプチャを利用できません。");
                return;
            }
        };
        tracing::info!(
            "キャプチャ {}x{} GLES 2.0：レンダリング {:?}",
            size.0,
            size.1,
            started.elapsed()
        );
        self.capture_reading.push(Reading {
            readback,
            copy,
            path,
            encoding,
            started,
        });
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn read_captures(&mut self) {
        if self.capture_reading.is_empty() {
            return;
        }
        let window = match self.painter.as_ref().map(|painter| painter.gles_window()) {
            Some(Ok(window)) => window,
            _ => {
                self.capture_reading.clear();
                return;
            }
        };
        let mut still = Vec::new();
        for mut reading in std::mem::take(&mut self.capture_reading) {
            match window.read_some(&mut reading.readback, READ_EACH_TURN) {
                Ok(None) => still.push(reading),
                Ok(Some(pixels)) => {
                    let size = reading.readback.size();
                    tracing::debug!("キャプチャ：読み戻しまで {:?}", reading.started.elapsed());
                    crate::capture::finish(
                        reading.copy,
                        reading.path,
                        reading.encoding,
                        crate::capture::Readback {
                            source: crate::capture::Source::Bytes(pixels),
                            stride: size.0 * 4,
                            size,
                            layout: crate::capture::Layout::Rgba8,
                        },
                        self.capture_said.0.clone(),
                    );
                }
                Err(error) => {
                    tracing::warn!("キャプチャ：{error:?}");
                    window.forget(&mut reading.readback);
                    self.toast("キャプチャを利用できません。");
                }
            }
        }
        self.capture_reading = still;
    }

    fn hand_over_captures(&mut self) {
        #[cfg(all(unix, not(target_os = "macos")))]
        self.read_captures();
        if self.capture_pending.is_empty() {
            return;
        }
        if let Some(Painter::Wgpu { gpu, .. }) = self.painter.as_ref() {
            let _ = gpu.device.poll(wgpu::PollType::Poll);
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.capture_pending)
            .into_iter()
            .partition(|pending| pending.state.load(std::sync::atomic::Ordering::Acquire) != 0);
        self.capture_pending = waiting;
        for pending in ready {
            if pending.state.load(std::sync::atomic::Ordering::Acquire) != 1 {
                self.toast("キャプチャを利用できません。");
                continue;
            }
            tracing::debug!("キャプチャ：読み戻しまで {:?}", pending.started.elapsed());
            crate::capture::finish(
                pending.copy,
                pending.path,
                pending.encoding,
                pending.readback,
                self.capture_said.0.clone(),
            );
        }
    }

    pub(super) fn tell_captures(&mut self) {
        self.hand_over_captures();
        while let Ok(said) = self.capture_said.1.try_recv() {
            self.toast(&said);
        }
        if zwcore::record::ended() {
            if let Some(done) = zwcore::record::stop() {
                let why = done.failed.unwrap_or_default();
                self.toast(&said_with("録画できませんでした。", &why));
            }
        } else if let Some(why) = zwcore::record::failure() {
            self.toast(&said_with("録画ファイルに書き込めません。", &why));
        }
    }

    pub(super) fn record(&mut self) {
        if let Some(done) = zwcore::record::stop() {
            self.toast(&stopped("録画を停止しました。", &done));
            return;
        }
        // 録画ファイルの再生中は、ファイルの中で見ているサービスを録る。
        let watching = self.player.tables().watching;
        let key = if self.player_mode {
            watching
        } else {
            watching.or(self.asked_key)
        };
        let Some(key) = key else {
            self.toast("録画できるサービスがありません。");
            return;
        };
        let file = crate::config::load_plain();
        let channel = self.status.channel.clone().unwrap_or_default();
        let event = self
            .player
            .tables()
            .now
            .as_ref()
            .map(|now| now.title.clone())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                self.guide
                    .at(key, self.guide.now_or_machine())
                    .map(|programme| programme.title.clone())
            })
            .unwrap_or_default();
        let event = crate::ui::text::badges::bracketed(&event);
        let folder = crate::capture::folder_of(&file.record.folder, crate::capture::Folder::Videos);
        let name = zwcore::record::file_name(
            zwcore::record::without_extension(&file.record.name),
            &channel,
            &event,
            file.guide.offset,
        );
        let path = folder.join(name);
        let options = zwcore::record::Options {
            service: file.record.service_only.then_some(key.service),
            caption: file.record.caption,
            carousel: file.record.carousel,
            eit: file.record.eit,
        };
        match zwcore::record::start(&path, options, file.record.buffers()) {
            Ok(()) => {
                self.reset_error_count();
                self.toast(&format!("録画を開始しました。\n{}", folder.display()));
            }
            Err(why) => self.toast(&said_with("録画を開始できません。", &why)),
        }
    }

    pub(super) fn stop_recording_for_switch(&mut self) {
        if let Some(done) = zwcore::record::stop() {
            self.toast(&stopped(
                "再生を切り替えたため、録画を停止しました。",
                &done,
            ));
        }
    }
}

use crate::capture::Job;

/// 録画を停止したときの通知。破棄した量と書き込めなかった理由が両方あるときはファイルの場所を省く。
pub(super) fn stopped(title: &str, done: &zwcore::record::Done) -> String {
    let mut lines = vec![title.to_owned()];
    if done.dropped > 0 {
        lines.push(format!(
            "書き込みが間に合わず、{:.1} MiB を破棄しました。",
            done.dropped as f64 / (1024.0 * 1024.0)
        ));
    }
    if let Some(why) = &done.failed {
        lines.push(format!("一部のデータを書き込めませんでした。{why}"));
    }
    if done.dropped == 0 || done.failed.is_none() {
        lines.push(done.path.display().to_string());
    }
    lines.join("\n")
}
