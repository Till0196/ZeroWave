// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 出力のコマンドを記録する（デインタレースから表示のパイプラインまで）と、フレームを置くスロット。

use super::*;

impl Video {
    pub(super) fn encode_output(&mut self, encoder: &mut wgpu::CommandEncoder) -> Option<Encoded> {
        let ready = self.schedule.front()?;
        let sub = self.sub;
        let per = self.front_per();
        let conversion = Conversion::new(
            self.settings.matrix,
            self.format.depth,
            self.settings.limited,
        )
        .adjusted(self.adjust, self.format.depth);
        let rows = filter::Rows::of(&ready.output);
        let params = Params {
            first_row: rows.first,
            mate_row: rows.mate,
            luma_offset: conversion.luma_offset,
            luma_gain: conversion.luma_gain,
            chroma_offset: conversion.chroma_offset,
            chroma_gain: conversion.chroma_gain,
            kr: conversion.kr,
            kb: conversion.kb,
            // The picture's size, not the texture's: hardware textures are padded to macroblocks.
            width: self.format.width,
            height: self.format.height,
            comb: matcher::Matcher::STRIPE_DEINT,
            motion: matcher::Matcher::MOTION,
            luma_lift: conversion.luma_lift,
            chroma_cos: conversion.chroma_cos,
            chroma_sin: conversion.chroma_sin,
        };
        self.queue.write_buffer(&self.params, 0, &params.bytes());
        // コーミングは film と判定していない間も数える（履歴が無ければ film と分からない）。
        let counting = self.counting() && counts(&ready.output) && sub == 0;
        if counting {
            self.ensure_matcher();
        }

        let textures = self.shape.textures() as usize;
        let mut bindings: Vec<wgpu::BindGroupEntry> = Vec::with_capacity(3 * textures + 1);
        let slots = [ready.before, ready.cur, ready.after];
        for plane in 0..textures {
            for (which, slot) in slots.iter().enumerate() {
                let views = &self.pool.slots[*slot].views;
                // A slot never written has no views (an output for a dropped frame); draw nothing.
                bindings.push(wgpu::BindGroupEntry {
                    binding: (plane * 3 + which) as u32,
                    resource: wgpu::BindingResource::TextureView(views.get(plane)?),
                });
            }
        }
        bindings.push(wgpu::BindGroupEntry {
            binding: self.shape.bindings(),
            resource: self.params.as_entire_binding(),
        });
        let matched = ready.output.matched && self.matcher.is_some();
        if matched {
            let matcher = self.matcher.as_ref().expect("checked");
            bindings.push(wgpu::BindGroupEntry {
                binding: self.shape.bindings() + 1,
                resource: matcher.combed.as_entire_binding(),
            });
        }
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planes"),
            layout: match self.matcher.as_ref() {
                Some(matcher) if matched => &matcher.layout,
                _ => &self.planes,
            },
            entries: &bindings,
        });

        if counting && let Some(matcher) = self.matcher.as_mut() {
            let luma = |slot: usize| self.pool.slots[slot].views.first();
            if let (Some(cur), Some(before)) = (luma(ready.cur), luma(ready.before)) {
                matcher.encode(
                    &self.device,
                    &self.queue,
                    encoder,
                    cur,
                    before,
                    rows.first,
                    self.slot_index[ready.cur],
                );
            }
        }
        // プラグインのデインタレースに渡すのは、フィールドを見せる普通の出力だけ。
        #[cfg(feature = "plugins")]
        let by_plugin = sub == 0
            && self.hooks.has_deinterlace()
            && ready.output.field.is_some()
            && ready.output.weave.is_none()
            && !matched;
        #[cfg(feature = "plugins")]
        let frame = super::hook::Frame {
            new_input: sub == 0,
            zw: self.hooks.next_zw(
                zwsdk::video::Zw {
                    field: match ready.output.field {
                        Some(Parity::Top) => 0,
                        Some(Parity::Bottom) => 1,
                        None => 2,
                    },
                    first_row: rows.first,
                    mate_row: rows.mate,
                    matched: u32::from(matched),
                    // フィールドの出力は 一つ目、二つ目、（rff なら）一つ目 の順。
                    second: u32::from(ready.output.field.is_some() && ready.output.at % 2 == 1),
                    ..Default::default()
                },
                self.phases
                    .front()
                    .and_then(|phases| phases.get(sub as usize))
                    .copied()
                    .unwrap_or(sub as f32 / per.max(1) as f32),
                per,
            ),
        };
        #[cfg(not(feature = "plugins"))]
        let by_plugin = false;
        // 補間の 2 回目の出力からは、フレームは 1 回目と同じ（`picture` に残っている）。
        if sub == 0 && !by_plugin {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("deinterlace"),
                color_attachments: &[Some(attachment(&self.picture_view))],
                ..Default::default()
            });
            pass.set_pipeline(match self.matcher.as_ref() {
                Some(matcher) if matched => &matcher.render,
                _ => &self.pipeline,
            });
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        #[cfg(feature = "plugins")]
        if by_plugin && let Some(frames) = self.hook_frames(&ready) {
            let ctx = super::hook::Ctx {
                device: &self.device,
                queue: &self.queue,
                key: self.hook_key([self.format.width, self.format.height]),
                picture: picture_format(self.format.depth),
            };
            let input = super::hook::Input {
                texture: &self.picture,
                view: &self.picture_view,
                size: [self.format.width, self.format.height],
            };
            // フィルターの開始は一番新しいフレームが揃った時刻（`hook::timing::Lead`）。
            let newest = self.hooks.frames().and_then(|[_, hi]| {
                let slot = ready.frames[(2 + hi) as usize];
                let uint = self.shape == Shape::Planar && self.format.depth > 8;
                self.pool.slots[slot].views.first().map(|view| (view, uint))
            });
            self.hooks.deinterlace(
                &ctx,
                encoder,
                &input,
                &self.picture_view,
                &frame,
                &frames,
                newest,
            );
        }
        if matched && sub == 0 {
            self.spent.matched += 1;
        }
        #[cfg(feature = "plugins")]
        let chained = {
            let ctx = super::hook::Ctx {
                device: &self.device,
                queue: &self.queue,
                key: self.hook_key([self.format.width, self.format.height]),
                picture: picture_format(self.format.depth),
            };
            let input = super::hook::Input {
                texture: &self.picture,
                view: &self.picture_view,
                size: [self.format.width, self.format.height],
            };
            self.hooks.picture(&ctx, encoder, &input, &frame)
        };
        #[cfg(not(feature = "plugins"))]
        let chained: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])> = None;
        let mut read: Vec<usize> = slots.to_vec();
        read.extend(ready.frames);
        read.sort_unstable();
        read.dedup();
        Some(Encoded {
            chained,
            read,
            sub,
            per,
            shown: Shown {
                field: ready.output.field,
                at: (u32::from(ready.output.at) * per + sub) as u8,
                of: (u32::from(ready.output.of) * per) as u8,
            },
        })
    }

    pub(super) fn encode_show(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: Option<(wgpu::TextureView, [u32; 2], wgpu::TextureFormat)>,
        onto: Option<(&wgpu::TextureView, Onto)>,
    ) {
        self.shown = source;
        if let Some((view, onto)) = onto {
            self.show(encoder, view, onto);
        }
    }

    pub(super) fn show(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        onto: Onto,
    ) {
        let (mut source, mut size, format) = self.shown.clone().unwrap_or_else(|| {
            (
                self.picture_view.clone(),
                [self.format.width, self.format.height],
                picture_format(self.format.depth),
            )
        });
        let rect = self.letterbox((onto.width, onto.height));
        let shown = [
            (rect[2] - rect[0]) / 2.0 * onto.width as f32,
            (rect[3] - rect[1]) / 2.0 * onto.height as f32,
        ];
        let key = self.blit_key(size, shown);
        self.prefetch_blit(key);
        // 大きく縮めるなら先に画素を残らず平均して縮める（[`reduce`]）。最近傍のときは通さない。
        if self.scaling.downscale != super::Scaler::Nearest
            && let Some(factor) = reduce::Reduce::factor(size, shown)
        {
            (source, size) = self.reduce.run(
                &self.device,
                &self.queue,
                encoder,
                &source,
                size,
                format,
                factor,
            );
        }
        let mapping = self.mapping();
        if key.pass == 2 {
            let first = self.blit_for(Blit { pass: 1, ..key });
            source = self.split.run(
                &self.device,
                &self.queue,
                encoder,
                &self.blits[first].1,
                &self.blit_layout,
                &self.sampler,
                &source,
                size,
                onto,
                rect,
                &mapping,
            );
        }
        // ガンマを外すのはここ。8 ビットの `picture` の手前でリニアにすると暗部が潰れる。
        self.queue.write_buffer(
            &self.fit,
            0,
            &shader::fit_bytes(rect, self.target.is_srgb(), &mapping, self.scaling.antiring),
        );
        let group = blit_group(
            &self.device,
            &self.blit_layout,
            &source,
            &self.sampler,
            &self.fit,
        );
        let at = self.blit_for(key);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("show"),
            color_attachments: &[Some(attachment(view))],
            ..Default::default()
        });
        pass.set_viewport(
            onto.left as f32,
            onto.top as f32,
            onto.width as f32,
            onto.height as f32,
            0.0,
            1.0,
        );
        if let Some([l, b, r, t]) = self.mask_box((onto.width, onto.height)) {
            let to_x = |at: f32| onto.left as f32 + (at + 1.0) / 2.0 * onto.width as f32;
            let to_y = |at: f32| onto.top as f32 + (1.0 - at) / 2.0 * onto.height as f32;
            let x0 = to_x(l).round().max(onto.left as f32) as u32;
            let x1 = to_x(r).round().min((onto.left + onto.width) as f32) as u32;
            let y0 = to_y(t).round().max(onto.top as f32) as u32;
            let y1 = to_y(b).round().min((onto.top + onto.height) as f32) as u32;
            if x1 > x0 && y1 > y0 {
                pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
            }
        }
        pass.set_pipeline(&self.blits[at].1);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..6, 0..1);
    }

    /// 大きさ `size` のフレームを `shown` 画素にコピーするときの方法（シェーダーと同じく 1.001 を超えたら縮小）。
    /// 分離可能なカーネルの縮小は横と縦に分ける（[`split`]、返すのは縦の二回目）。
    fn blit_key(&self, size: [u32; 2], shown: [f32; 2]) -> Blit {
        let ratio = (size[0] as f32 / shown[0].max(1.0)).max(size[1] as f32 / shown[1].max(1.0));
        if ratio > 1.001 {
            let down = self.scaling.downscale.code();
            Blit {
                up: 0,
                down,
                pass: if (2..=5).contains(&down) { 2 } else { 0 },
            }
        } else {
            Blit {
                up: self.scaling.upscale.code(),
                down: 0,
                pass: 0,
            }
        }
    }

    fn blit_for(&mut self, key: Blit) -> usize {
        if let Some(at) = self.blits.iter().position(|(have, _)| *have == key) {
            return at;
        }
        if let Some(at) = self.blits_coming.iter().position(|(have, _)| *have == key) {
            let (_, coming) = self.blits_coming.swap_remove(at);
            if let Ok(pipeline) = coming.recv() {
                self.blits.push((key, pipeline));
                return self.blits.len() - 1;
            }
        }
        let pipeline = sheet::build_specialised(
            &self.device,
            &self.blit_layout,
            shader::blit().to_string(),
            if key.pass == 1 {
                split::FORMAT
            } else {
                self.target
            },
            "blit",
            &[
                ("UP", f64::from(key.up)),
                ("DOWN", f64::from(key.down)),
                ("PASS", f64::from(key.pass)),
            ],
        );
        self.blits.push((key, pipeline));
        self.blits.len() - 1
    }

    /// いまと逆向きの表示のパイプラインを裏でビルドしておく（初めて要るときに映像が止まらないよう）。
    fn prefetch_blit(&mut self, now: Blit) {
        let other = if now.down != 0 {
            Blit {
                up: self.scaling.upscale.code(),
                down: 0,
                pass: 0,
            }
        } else {
            let down = self.scaling.downscale.code();
            Blit {
                up: 0,
                down,
                pass: if (2..=5).contains(&down) { 2 } else { 0 },
            }
        };
        if self.blits.iter().any(|(have, _)| *have == other)
            || self.blits_coming.iter().any(|(have, _)| *have == other)
        {
            return;
        }
        let (to, from) = std::sync::mpsc::channel();
        let device = self.device.clone();
        let layout = self.blit_layout.clone();
        let target = self.target;
        let spawned = std::thread::Builder::new()
            .name(String::from("blit-prefetch"))
            .spawn(move || {
                let pipeline = sheet::build_specialised(
                    &device,
                    &layout,
                    shader::blit().to_string(),
                    target,
                    "blit",
                    &[
                        ("UP", f64::from(other.up)),
                        ("DOWN", f64::from(other.down)),
                        ("PASS", f64::from(other.pass)),
                    ],
                );
                let _ = to.send(pipeline);
            });
        if spawned.is_ok() {
            self.blits_coming.push((other, from));
        }
    }

    pub(super) fn finish_output(&mut self, encoded: Encoded, consume: bool) -> Shown {
        self.drawn_once = true;
        // film と 60i が替わっても、いま保持しているスケジュールはそのまま（次のフレームから替わる）。
        if let Some(matcher) = self.matcher.as_mut()
            && matcher.finish(&self.device)
        {
            self.spent.film_changes += 1;
        }
        // 読んだフレームは完了の通知までデコーダーに返さない（スロットの `keep` は使い回すとコマンドを記録した直後に落ちる）。
        let kept: Vec<std::sync::Arc<dyn std::any::Any + Send + Sync>> = encoded
            .read
            .iter()
            .filter_map(|slot| self.pool.slots[*slot].keep.clone())
            .collect();
        if !kept.is_empty() {
            self.queue.on_submitted_work_done(move || {
                drop(kept);
            });
        }
        if consume {
            if encoded.sub + 1 < encoded.per {
                self.sub += 1;
            } else {
                self.sub = 0;
                self.pop_output();
            }
        }
        encoded.shown
    }

    /// Where the picture sits in a display of this size, in clip coordinates (aspect from the header).
    pub(super) fn letterbox(&self, size: (u32, u32)) -> [f32; 4] {
        self.boxes(size).1
    }

    fn mask_box(&self, size: (u32, u32)) -> Option<[f32; 4]> {
        (!self.inset_shrinks() && self.pan_scan.mask && !self.pan_scan.is_default())
            .then(|| self.boxes(size).0)
    }

    /// データ放送の文書が映像を一部の領域へ縮小しているか（画面いっぱいなら比率は効かせる）。
    fn inset_shrinks(&self) -> bool {
        self.inset.is_some_and(|[left, top, wide, tall, _]| {
            left > 0.001 || top > 0.001 || wide < 0.999 || tall < 0.999
        })
    }

    fn boxes(&self, (width, height): (u32, u32)) -> ([f32; 4], [f32; 4]) {
        if !self.inset_shrinks() && !self.pan_scan.is_default() {
            let (num, den) = self.format.aspect;
            let source = if num == 0 || den == 0 {
                self.format.width as f32 / self.format.height.max(1) as f32
            } else {
                num as f32 / den as f32
            };
            let have = width.max(1) as f32 / height.max(1) as f32;
            return self.pan_scan.fit(source, have);
        }
        let rect = self.letterbox_plain((width, height));
        (rect, rect)
    }

    fn letterbox_plain(&self, (width, height): (u32, u32)) -> [f32; 4] {
        let (num, den) = self.format.aspect;
        // With an inset the box is the data broadcast's page, whose shape the inset names.
        let wanted = match self.inset {
            Some([_, _, _, _, aspect]) if aspect > 0.0 => aspect,
            _ if num == 0 || den == 0 => {
                self.format.width as f32 / self.format.height.max(1) as f32
            }
            _ => num as f32 / den as f32,
        };
        let have = width.max(1) as f32 / height.max(1) as f32;
        // 枠がフレームと同じ比率なら枠いっぱい（丸めの差の一画素で黒い帯を作らない）。
        let (x, y) = if (have - wanted).abs() / wanted < 0.01 {
            (1.0, 1.0)
        } else if have > wanted {
            (wanted / have, 1.0)
        } else {
            (1.0, have / wanted)
        };
        let Some([left, top, wide, tall, _]) = self.inset else {
            return [-x, -y, x, y];
        };
        // The inset is measured from the top left of the letterbox; clip space y goes up.
        let l = -x + 2.0 * x * left;
        let t = y - 2.0 * y * top;
        [l, t - 2.0 * y * tall, l + 2.0 * x * wide, t]
    }
}
