// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 次の出力をレンダリングする入口、プラグインのチェーンと先読みのキュー、読み戻し。

use super::*;

impl Video {
    /// デバイスを一度 poll する。レンダリング完了の通知はこれをしないと届かないので毎回呼ぶ。
    pub fn poll(&self) {
        self.device.poll(wgpu::PollType::Poll).ok();
    }

    pub fn render(
        &mut self,
        view: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Result<Option<Shown>, Error> {
        Ok(self.draw_one(Some((view, Onto::all(size)))))
    }

    pub fn present(&mut self, surface: &wgpu::Surface<'_>) -> Result<Option<Shown>, Error> {
        if !self.ready() {
            return Ok(None);
        }
        // A suboptimal surface still holds a picture; the caller reconfigures when it is ready.
        let began = Instant::now();
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => return Err(Error::Surface(format!("{other:?}"))),
        };
        let got = Instant::now();
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let size = (frame.texture.width(), frame.texture.height());
        let shown = self.draw_one(Some((&view, Onto::all(size))));
        let drew = Instant::now();
        self.queue.present(frame);
        self.spent.acquire += got - began;
        self.spent.draw += drew - got;
        self.spent.show += drew.elapsed();
        self.spent.shown += u64::from(shown.is_some());
        Ok(shown)
    }

    /// 用意されたサーフェスへ 1 フレームをレンダリングする（取得も提示も呼び出し側）。レンダリングするものが無ければ `None`。
    pub fn draw_onto(&mut self, view: &wgpu::TextureView, onto: Onto) -> Option<Shown> {
        if !self.ready() {
            return None;
        }
        let began = Instant::now();
        let shown = self.draw_one(Some((view, onto)));
        self.spent.draw += began.elapsed();
        self.spent.shown += u64::from(shown.is_some());
        shown
    }

    fn draw_one(&mut self, onto: Option<(&wgpu::TextureView, Onto)>) -> Option<Shown> {
        self.draw(onto, true)
    }

    /// 前にレンダリングしたフレームをもう一度サーフェスへコピーする（無ければ黒）。取得したサーフェスは必ず提示する決まり（DXGI）
    /// で、何もレンダリングせずに提示すると前のフレームが残ってちらつく。
    pub fn redraw_onto(&mut self, view: &wgpu::TextureView, onto: Onto) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("show again"),
            });
        if self.drawn_once {
            self.show(&mut encoder, view, onto);
        } else {
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("show again"),
                color_attachments: &[Some(attachment(view))],
                ..Default::default()
            }));
        }
        self.queue.submit([encoder.finish()]);
        self.spent.shown_again += 1;
    }

    /// [`Video::redraw_onto`] と同じだが、プラグインのチェーンを通す前のフレームでレンダリングする（キャプチャ用）。
    pub fn redraw_unfiltered_onto(&mut self, view: &wgpu::TextureView, onto: Onto) {
        let shown = self.shown.take();
        self.redraw_onto(view, onto);
        self.shown = shown;
    }

    /// 一つの出力を何回レンダリングするか（プラグインのチェーンのフレーム補間。無ければ 1）。
    pub(super) fn outputs_per(&self) -> u32 {
        #[cfg(feature = "plugins")]
        return self.hooks.outputs();
        #[cfg(not(feature = "plugins"))]
        1
    }

    /// プラグインのフィルターを要ればビルドし直し、届くフレームの間隔が変わればスケジュールを立て直す。
    #[cfg(feature = "plugins")]
    fn refresh_hooks(&mut self, target: [u32; 2]) {
        let ctx = super::hook::Ctx {
            device: &self.device,
            queue: &self.queue,
            key: self.hook_key(target),
            picture: picture_format(self.format.depth),
        };
        self.hooks.refresh(&ctx);
        let reach = self.hooks.reach();
        if reach != self.schedule.reach() {
            self.lose(self.pers.len());
            self.schedule = Schedule::with_reach(self.pool.slots.len(), reach.0, reach.1);
            self.last_slot = None;
        }
    }

    #[cfg(feature = "plugins")]
    pub(super) fn hook_key(&self, target: [u32; 2]) -> super::hook::Key {
        use super::colour::Transfer;
        super::hook::Key {
            width: self.format.width,
            height: self.format.height,
            depth: self.format.depth,
            shape: self.shape,
            interlaced: !(self.format.progressive && crate::output::video::adapt_progressive()),
            aspect: [self.format.aspect.0, self.format.aspect.1],
            rate: [self.format.rate.0, self.format.rate.1],
            transfer: match self.settings.transfer {
                Transfer::Sdr => "sdr",
                Transfer::Hlg => "hlg",
                Transfer::Pq => "pq",
                Transfer::Ui => "linear",
            },
            target,
            display: self.display_mhz,
        }
    }

    #[cfg(feature = "plugins")]
    pub(super) fn hook_frames(&self, ready: &super::schedule::Ready) -> Option<wgpu::BindGroup> {
        let layout = self.hooks.frames_layout()?;
        let [lo, hi] = self.hooks.frames()?;
        let count = (hi - lo + 1) as usize;
        let mut entries = Vec::new();
        for plane in 0..self.shape.textures() as usize {
            for frame in lo..=hi {
                let slot = ready.frames[(2 + frame) as usize];
                entries.push(wgpu::BindGroupEntry {
                    binding: (plane * count + (frame - lo) as usize) as u32,
                    resource: wgpu::BindingResource::TextureView(
                        self.pool.slots[slot].views.get(plane)?,
                    ),
                });
            }
        }
        entries.push(wgpu::BindGroupEntry {
            binding: (self.shape.textures() as usize * count) as u32,
            resource: self.params.as_entire_binding(),
        });
        Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zw frames"),
            layout,
            entries: &entries,
        }))
    }

    #[cfg(feature = "plugins")]
    fn target_size(&self, onto: Option<Onto>) -> [u32; 2] {
        match onto {
            Some(onto) => {
                let [l, b, r, t] = self.letterbox((onto.width, onto.height));
                [
                    (((r - l) * 0.5 * onto.width as f32).round() as u32).max(1),
                    (((t - b) * 0.5 * onto.height as f32).round() as u32).max(1),
                ]
            }
            None => [self.format.width, self.format.height],
        }
    }

    pub(super) fn draw(
        &mut self,
        onto: Option<(&wgpu::TextureView, Onto)>,
        consume: bool,
    ) -> Option<Shown> {
        #[cfg(feature = "plugins")]
        self.refresh_hooks(self.target_size(onto.map(|(_, onto)| onto)));
        // 先読みしているならキューの先頭をコピーするだけ（やめてもキューにある分は出し切る）。
        #[cfg(feature = "plugins")]
        if consume && (self.ahead_depth() > 0 || !self.ahead.ring.is_empty()) {
            return self.draw_ahead(onto);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("show"),
            });
        let encoded = self.encode_output(&mut encoder)?;
        #[cfg(feature = "plugins")]
        let source = encoded
            .chained
            .as_ref()
            .map(|(_, view, size)| (view.clone(), *size, super::hook::WORKING));
        #[cfg(not(feature = "plugins"))]
        let source: Option<(wgpu::TextureView, [u32; 2], wgpu::TextureFormat)> = None;
        self.encode_show(&mut encoder, source, onto);
        self.queue.submit([encoder.finish()]);
        #[cfg(feature = "plugins")]
        self.hooks.after_submit();
        Some(self.finish_output(encoded, consume))
    }

    /// チェーンに新しい入力が来る間隔（ミリ秒）。まだ 1 フレームも来ていなければフィールドの間隔。
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub(super) fn content_interval_ms(&self) -> f64 {
        if self.content.1 > 0.0 && self.content.0 > 0.0 {
            1000.0 * self.content.0 / self.content.1
        } else {
            super::FIELD_BUDGET_MS
        }
    }

    /// 先読みするフレーム数（0 なら先読みしない）。[`AHEAD_MEMORY`] で抑える。
    #[cfg(feature = "plugins")]
    fn ahead_depth(&self) -> usize {
        let ms = self.hooks.buffer_ms();
        if ms == 0 || self.hooks.is_empty() {
            return 0;
        }
        let frames = (f64::from(ms) / self.content_interval_ms()).ceil().max(1.0) as usize;
        let [w, h] = self
            .hooks
            .output_size()
            .unwrap_or([self.format.width, self.format.height]);
        let bytes = u64::from(w.max(1)) * u64::from(h.max(1)) * 8;
        frames.min((AHEAD_MEMORY / bytes).max(1) as usize)
    }

    /// 一回の呼び出しで先に作ってよいフレーム数（1〜3、[`super::HEADROOM`] に入る数）。
    #[cfg(feature = "plugins")]
    fn ahead_per_draw(&self) -> usize {
        let Some(spent) = self.hooks.spent_ms().filter(|ms| *ms > 0.0) else {
            return 1;
        };
        ((self.content_interval_ms() * super::HEADROOM / spent).floor() as usize).clamp(1, 3)
    }

    /// 一つ作り足してからキューの先頭をコピーする。作るのを先に投入する（コピーの待ち時間をチェーンの負荷に数えない）。
    #[cfg(feature = "plugins")]
    fn draw_ahead(&mut self, onto: Option<(&wgpu::TextureView, Onto)>) -> Option<Shown> {
        let depth = self.ahead_depth();
        if (self.ahead.ring.len() < depth || self.ahead.ring.is_empty())
            && !self.produce()
            && self.ahead.ring.is_empty()
        {
            return None;
        }
        let made = self.ahead.ring.pop_front()?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("show ahead"),
            });
        self.encode_show(
            &mut encoder,
            Some((made.view.clone(), made.size, made.format)),
            onto,
        );
        self.queue.submit([encoder.finish()]);
        self.drawn_once = true;
        let shown = made.shown;
        if let Some(old) = self.ahead.showing.replace(made) {
            self.ahead.spare.push(old);
        }
        for _ in 1..self.ahead_per_draw() {
            if self.ahead.ring.len() >= depth || !self.produce() {
                break;
            }
        }
        self.ahead.spare.truncate(depth.max(1));
        Some(shown)
    }

    #[cfg(feature = "plugins")]
    fn produce(&mut self) -> bool {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ahead"),
            });
        let Some(encoded) = self.encode_output(&mut encoder) else {
            return false;
        };
        let (texture, size, format) = match &encoded.chained {
            Some((texture, _, size)) => (texture.clone(), *size, super::hook::WORKING),
            None => (
                self.picture.clone(),
                [self.format.width, self.format.height],
                picture_format(self.format.depth),
            ),
        };
        let at = self
            .ahead
            .spare
            .iter()
            .position(|made| made.size == size && made.format == format);
        let made = match at {
            Some(at) => self.ahead.spare.swap_remove(at),
            None => {
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("ahead"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                Made {
                    texture,
                    view,
                    size,
                    format,
                    shown: encoded.shown,
                }
            }
        };
        encoder.copy_texture_to_texture(
            texture.as_image_copy(),
            made.texture.as_image_copy(),
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        self.hooks.after_submit();
        let shown = self.finish_output(encoded, true);
        self.ahead.ring.push_back(Made { shown, ..made });
        true
    }

    pub fn read_back(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        self.draw_one(None)?;
        self.snapshot()
    }

    pub fn snapshot(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let (width, height) = (self.format.width, self.format.height);
        let stride = padded(width * 4);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: u64::from(stride * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("read back"),
            });
        // 直に読んで描いたフレームは RGB を作っていない。
        self.convert_direct(&mut encoder);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.picture,
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
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let view = buffer.get_mapped_range(..).ok()?;
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height as usize {
            let at = y * stride as usize;
            let row = &view[at..at + (width * 4) as usize];
            if self.format.depth > 8 {
                // Ten bits per channel, red lowest; rounded as `colour::quantise` does.
                for word in row.as_chunks::<4>().0 {
                    let packed = u32::from_le_bytes(*word);
                    let eight = |ten: u32| ((ten * 255 * 2 + 1023) / 2046) as u8;
                    out.push(eight(packed & 0x3ff));
                    out.push(eight((packed >> 10) & 0x3ff));
                    out.push(eight((packed >> 20) & 0x3ff));
                    out.push(255);
                }
            } else {
                out.extend_from_slice(row);
            }
        }
        drop(view);
        buffer.unmap();
        Some((out, width, height))
    }
}
