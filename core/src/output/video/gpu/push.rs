// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードしたフレームを受け取ってスケジュールに入れる、取り込み、破棄、コーミングを数える。

use super::*;

type Imported = (
    Vec<wgpu::TextureView>,
    Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
);

impl Video {
    /// Takes one decoded frame and says how many outputs it became; callers must not recompute it.
    /// Fails with [`Error::Full`] when every slot is spoken for.
    pub fn push(&mut self, picture: &Decoded) -> Result<usize, Error> {
        self.push_at(picture, None)
    }

    /// [`Video::push`] に、そのフレームがストリームの何フレーム目かを添えて（3:2 の位相は番号で判定する）。
    pub fn push_at(&mut self, picture: &Decoded, number: Option<u64>) -> Result<usize, Error> {
        let mut about = picture.about();
        let shape = match picture {
            Decoded::Planes(_) => Shape::Planar,
            Decoded::Surface(_) => Shape::Biplanar,
        };
        if shape != self.shape {
            self.rebuild(self.format, self.settings, shape)?;
        }
        let woven = about.format.field_pictures && self.format.woven;
        let mut pair = None;
        if woven {
            let Decoded::Surface(surface) = picture else {
                return Err(Error::Unsupported("a lone field in main memory"));
            };
            let began = Instant::now();
            let (views, keep) = self.imported(surface, (self.format.width, about.format.height))?;
            self.spent.upload += began.elapsed();
            let top = match about.structure {
                mediadecoder::es::Structure::Top => true,
                mediadecoder::es::Structure::Bottom => false,
                mediadecoder::es::Structure::Frame => {
                    self.waiting.as_ref().is_none_or(|first| !first.top)
                }
            };
            let field = weave::Field { views, keep, top };
            // 同じパリティーが続いたら先のフィールドは捨てる（[`crate::decode::weave::Weaver`] と同じ）。
            match self.waiting.take() {
                Some(first) if first.top != top && self.schedule.free().is_some() => {
                    pair = Some((first, field));
                }
                Some(first) if first.top != top => {
                    self.waiting = Some(first);
                    return Err(Error::Full);
                }
                _ => {
                    self.waiting = Some(field);
                    self.last_phases.clear();
                    return Ok(0);
                }
            }
        }
        let slot = self.schedule.free().ok_or(Error::Full)?;
        let began = Instant::now();
        let index = match number {
            Some(number) if woven => number / 2,
            Some(number) => number,
            None => self.pushed,
        };
        self.pushed += 1;
        let before = self.last_slot.replace(slot);
        self.slot_index[slot] = index;
        if let Some((first, second)) = pair {
            about = crate::decode::About {
                format: self.format,
                structure: mediadecoder::es::Structure::Frame,
                top_field_first: first.top,
                repeat_first_field: false,
                progressive: false,
                ..about
            };
            self.device.poll(wgpu::PollType::Poll).ok();
            self.weave_into(slot, first, second);
        } else {
            match picture {
                Decoded::Planes(frame) => {
                    if frame.planes[0].width != self.format.width as usize
                        || frame.planes[0].height != self.format.height as usize
                    {
                        return Err(Error::Unsupported("the picture changed size"));
                    }
                    self.pool.write(
                        &self.device,
                        &self.queue,
                        slot,
                        frame,
                        self.upload,
                        &mut self.scratch,
                    );
                    self.device.poll(wgpu::PollType::Poll).ok();
                }
                Decoded::Surface(surface) => {
                    // レンダリング完了の通知（`on_submitted_work_done`）は poll しないと届かない。
                    self.device.poll(wgpu::PollType::Poll).ok();
                    let (views, keep) =
                        self.imported(surface, (self.format.width, self.format.height))?;
                    self.pool.adopt(slot, views, keep);
                }
            }
        }
        self.spent.upload += began.elapsed();
        self.spent.pushed += 1;
        // プログレッシブはそのまま。film はフィールドを組み合わせ、周期が分かれば 5 フレームに 1 フレームを抜く。60i は候補の方式から選ぶ。
        let judge = self
            .matcher
            .as_ref()
            .filter(|_| self.counting())
            .map(matcher::Matcher::judge);
        self.last_progressive = about.progressive;
        let plan = if about.progressive
            && crate::output::video::adapt_progressive()
            && self.settings.deinterlace != Deinterlace::Weave
        {
            field::plan(&about, Deinterlace::Weave)
        } else if judge.is_some_and(|judge| judge.matching()) {
            let cadence = if judge.is_some_and(|judge| judge.drop(index)) {
                self.spent.woven += 1;
                // 抜くフレームも数える（繰り返しのフィールドが周期の手がかり）。
                if let Some(before) = before {
                    self.count_only(slot, before, field::first_field(&about).first_row(), index);
                }
                Cadence::Drop
            } else {
                Cadence::None
            };
            field::plan_matched(&about, cadence)
        } else {
            field::plan(&about, self.settings.deinterlace)
        };
        let outputs = self.plan_outputs(plan.len());
        // ウィーブしたフレームの出力の位置は、後のフィールドの時刻からフィールドの長さで数える（先のフィールドは -1）。
        if woven {
            for phase in &mut self.last_phases {
                *phase = *phase * 2.0 - 1.0;
            }
        }
        self.schedule.hold(slot, plan);
        Ok(outputs)
    }

    fn weave_into(&mut self, slot: usize, first: weave::Field, second: weave::Field) {
        let (top, bottom) = if first.top {
            (&first, &second)
        } else {
            (&second, &first)
        };
        let Some(into) = self.pool.owned(slot) else {
            return;
        };
        let weave = self
            .weave
            .get_or_insert_with(|| weave::Weave::new(&self.device));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("weave"),
            });
        weave.run(&self.device, &mut encoder, &top.views, &bottom.views, into);
        self.queue.submit([encoder.finish()]);
        // 読み終えるまでデコーダーにフィールドのテクスチャを使わせない。
        let kept: Vec<std::sync::Arc<dyn std::any::Any + Send + Sync>> =
            [first.keep, second.keep].into_iter().flatten().collect();
        if !kept.is_empty() {
            self.queue.on_submitted_work_done(move || {
                drop(kept);
            });
        }
    }

    /// 一フレームの出力を何回・どの位相でレンダリングするか決めてスケジュールに入れ、出力回数の合計を返す。
    /// 一回も来ない出力も一回はレンダリングする（前後のフレームの履歴を進めるため）。
    fn plan_outputs(&mut self, entries: usize) -> usize {
        self.last_phases.clear();
        let per = self.outputs_per();
        #[cfg(feature = "plugins")]
        let retime = self.hooks.retime();
        #[cfg(not(feature = "plugins"))]
        let retime: Option<f64> = None;
        let (num, den) = self.format.rate;
        let frame = if num > 0 && den > 0 {
            f64::from(den) / f64::from(num)
        } else {
            1001.0 / 30000.0
        };
        self.content = (
            self.content.0 * 0.9 + frame,
            self.content.1 * 0.9 + entries as f64,
        );
        #[cfg(feature = "plugins")]
        self.hooks.set_interval(self.content_interval_ms());
        let mut total = 0;
        for entry in 0..entries {
            let phases: Vec<f32> = match retime {
                Some(rate) => retime_phases(&mut self.retime_at, rate * frame / entries as f64),
                None => (0..per).map(|sub| sub as f32 / per as f32).collect(),
            };
            total += phases.len();
            // フレームレート変換したフレームは「前の入力 → この入力」の間の物なので、一つ前の入力の時刻から並べる。
            let shift = if retime.is_some() { 1.0 } else { 0.0 };
            self.last_phases.extend(
                phases
                    .iter()
                    .map(|p| (entry as f32 + p - shift) / entries as f32),
            );
            self.pers.push_back(phases.len() as u32);
            self.phases.push_back(phases);
        }
        total
    }

    /// 最後の `push` の出力がそのフレームの中のどこに出るか（フレームの長さで数えた位置。負にもなる）。
    pub fn last_phases(&self) -> &[f32] {
        &self.last_phases
    }

    /// 画面のリフレッシュレート（ミリヘルツ）。フレーム補間の目標になる。
    pub fn set_display_rate(&mut self, mhz: u32) {
        self.display_mhz = mhz;
    }

    #[cfg(feature = "vaapi")]
    fn imported(
        &mut self,
        surface: &crate::decode::Surface,
        size: (u32, u32),
    ) -> Result<Imported, Error> {
        let crate::decode::Held::DmaBuf(buffer) = &surface.held;
        if (buffer.width, buffer.height) != size {
            return Err(Error::Unsupported("the picture changed size"));
        }
        let import = self
            .import
            .as_mut()
            .ok_or(Error::Unsupported("a device that cannot import a frame"))?;
        let views = import.views(buffer)?;
        if views.len() != self.shape.textures() as usize {
            return Err(Error::Unsupported("a frame of the wrong number of planes"));
        }
        // スロットがこのフレームを持つ間、デコーダーにそのバッファを使わせない（V4L2 のみ。VA-API には無い）。
        Ok((views, buffer.lease.clone()))
    }

    #[cfg(all(
        feature = "d3d12",
        windows,
        not(feature = "vaapi"),
        not(all(feature = "android", target_os = "android"))
    ))]
    fn imported(
        &mut self,
        surface: &crate::decode::Surface,
        size: (u32, u32),
    ) -> Result<Imported, Error> {
        let crate::decode::Held::D3d12(frame) = &surface.held;
        if (frame.width, frame.height) != size {
            return Err(Error::Unsupported("the picture changed size"));
        }
        let import = self
            .import
            .as_mut()
            .ok_or(Error::Unsupported("a device that cannot import a frame"))?;
        let (views, keep) = import.take(frame, &self.queue)?;
        if views.len() != self.shape.textures() as usize {
            return Err(Error::Unsupported("a frame of the wrong number of planes"));
        }
        Ok((views, keep))
    }

    #[cfg(all(feature = "android", target_os = "android"))]
    fn imported(
        &mut self,
        surface: &crate::decode::Surface,
        size: (u32, u32),
    ) -> Result<Imported, Error> {
        let crate::decode::Held::HardwareBuffer(frame) = &surface.held else {
            return Err(Error::Unsupported("a picture of another platform"));
        };
        if (frame.width, frame.height) != size {
            return Err(Error::Unsupported("the picture changed size"));
        }
        let import = self
            .import
            .as_mut()
            .ok_or(Error::Unsupported("a device that cannot import a frame"))?;
        let views = import.views(frame)?;
        if views.len() != self.shape.textures() as usize {
            return Err(Error::Unsupported("a frame of the wrong number of planes"));
        }
        Ok((views, None))
    }

    #[cfg(all(
        feature = "videotoolbox",
        target_vendor = "apple",
        not(feature = "vaapi"),
        not(all(feature = "d3d12", windows)),
        not(all(feature = "android", target_os = "android"))
    ))]
    fn imported(
        &mut self,
        surface: &crate::decode::Surface,
        size: (u32, u32),
    ) -> Result<Imported, Error> {
        let crate::decode::Held::VideoToolbox(frame) = &surface.held else {
            return Err(Error::Unsupported("a picture of another platform"));
        };
        if (frame.width, frame.height) != size {
            return Err(Error::Unsupported("the picture changed size"));
        }
        let import = self
            .import
            .as_mut()
            .ok_or(Error::Unsupported("a device that cannot import a frame"))?;
        let (views, keep) = import.views(frame)?;
        if views.len() != self.shape.textures() as usize {
            return Err(Error::Unsupported("a frame of the wrong number of planes"));
        }
        // スロットが読む間はピクセルバッファを保持する（VideoToolbox が使い回してちらつかないよう）。
        Ok((views, Some(keep)))
    }

    #[cfg(not(any(
        feature = "vaapi",
        all(feature = "videotoolbox", target_vendor = "apple"),
        all(feature = "d3d12", windows),
        all(feature = "android", target_os = "android")
    )))]
    fn imported(
        &mut self,
        surface: &crate::decode::Surface,
        size: (u32, u32),
    ) -> Result<Imported, Error> {
        let _ = (surface, size);
        Err(Error::Unsupported("a picture the hardware still owns"))
    }

    /// Says the stream has ended, so the last frame is shown instead of waiting for the next.
    pub fn flush(&mut self) {
        self.schedule.expand(true);
    }

    pub fn reset(&mut self) {
        #[cfg(feature = "plugins")]
        {
            let ahead = &mut self.ahead;
            ahead.spare.extend(ahead.ring.drain(..));
        }
        self.content = (0.0, 0.0);
        self.schedule.clear();
        self.waiting = None;
        self.last_slot = None;
        self.sub = 0;
        self.pers.clear();
        self.phases.clear();
        self.lost = 0;
        // The frame before is not the frame before any more, so the cadence is forgotten.
        if let Some(matcher) = self.matcher.as_mut() {
            matcher.judge_mut().reset();
        }
        // 前のチャンネルのサーフェスのラッパーを解放する（作り直したサーフェスはキーが違い、残すと VRAM が増え続ける）。
        #[cfg(any(
            feature = "vaapi",
            all(feature = "android", target_os = "android"),
            all(feature = "d3d12", windows),
            all(feature = "videotoolbox", target_vendor = "apple")
        ))]
        if let Some(import) = self.import.as_mut() {
            import.forget();
        }
    }

    pub fn spent(&mut self) -> Spent {
        let mut spent = std::mem::take(&mut self.spent);
        spent.soft = self.last_progressive;
        if let Some(judge) = self.matcher.as_mut().map(matcher::Matcher::judge_mut) {
            spent.locked = judge.locked();
            spent.whole_frames = judge.whole_frames();
            spent.film = judge.matching();
            spent.overlay = judge.overlay();
            spent.combed = judge.combed();
            spent.stripes = judge.stripes();
        }
        spent
    }

    pub fn pending(&self) -> usize {
        self.schedule.len() + self.ahead_len()
    }

    pub fn ready(&self) -> bool {
        !self.schedule.is_empty() || self.ahead_len() > 0
    }

    fn ahead_len(&self) -> usize {
        #[cfg(feature = "plugins")]
        return self.ahead.ring.len();
        #[cfg(not(feature = "plugins"))]
        0
    }

    /// Throws the next output away without drawing it, and says whether there was one.
    pub fn skip(&mut self) -> bool {
        #[cfg(feature = "plugins")]
        if let Some(made) = self.ahead.ring.pop_front() {
            self.ahead.spare.push(made);
            return true;
        }
        // 補間の途中なら、そのフレームだけを飛ばす（スケジュールは一つの出力のまま）。
        if self.schedule.front().is_some() && self.sub + 1 < self.front_per() {
            self.sub += 1;
            return true;
        }
        self.sub = 0;
        // 飛ばすフレームもコーミングの履歴には入れる（周期の判定を欠かさない）。
        if let Some(ready) = self.schedule.front()
            && counts(&ready.output)
            && self.counting()
        {
            let first_row = filter::Rows::of(&ready.output).first;
            self.count_only(
                ready.cur,
                ready.before,
                first_row,
                self.slot_index[ready.cur],
            );
        }
        self.pop_output()
    }

    /// スケジュールの先頭から `entries` 個の出力を、レンダリングせずに捨てたと数える。
    pub(super) fn lose(&mut self, entries: usize) {
        let entries = entries.min(self.pers.len());
        let mut count: usize = self.pers.drain(..entries).map(|per| per as usize).sum();
        self.phases.drain(..entries.min(self.phases.len()));
        if entries > 0 {
            count = count.saturating_sub(self.sub as usize);
        }
        self.lost += count;
        self.sub = 0;
    }

    #[cfg(feature = "plugins")]
    pub fn filter_report(&self) -> super::hook::Report {
        self.hooks.report()
    }

    /// 映像フィルターがフレームを遅らせて出す長さ（ミリ秒、[`super::hook::Hooks::held_ms`]）。
    #[cfg(feature = "plugins")]
    pub fn filter_held_ms(&self) -> f64 {
        self.hooks.held_ms()
    }

    #[cfg(feature = "plugins")]
    pub fn wait_filters(&mut self) {
        self.hooks.wait_built();
    }

    #[cfg(feature = "plugins")]
    pub fn set_filter_program(&mut self, program: Option<zwsdk::video::Program>) {
        self.hooks.set_program(program);
    }

    pub fn set_scaling(&mut self, scaling: super::Scaling) {
        self.asked_scaling = scaling;
        self.scaling = scaling.resolved(self.strength);
    }

    /// 方式を自動のままシェーダーでデインタレースしたか、拡大縮小を自動にしている（GPU を測る意味がある）。
    pub fn wants_measure(&self) -> bool {
        self.shaded
    }

    /// 測った GPU で間に合うデインタレース。自動の拡大縮小をこれで決める。
    pub fn set_strength(&mut self, strength: Option<Deinterlace>) {
        self.strength = strength;
        self.scaling = self.asked_scaling.resolved(strength);
    }

    pub fn set_adjust(&mut self, adjust: crate::output::video::Adjust) {
        self.adjust = adjust;
    }

    /// レンダリングせずに捨てた出力の数を返して数え直す。
    pub fn take_lost(&mut self) -> usize {
        std::mem::take(&mut self.lost)
    }

    pub(super) fn front_per(&self) -> u32 {
        self.pers.front().copied().unwrap_or(1)
    }

    pub(super) fn pop_output(&mut self) -> bool {
        self.pers.pop_front();
        self.phases.pop_front();
        self.schedule.pop()
    }

    pub(super) fn ensure_matcher(&mut self) {
        if self.matcher.is_some() {
            return;
        }
        self.matcher = Some(matcher::Matcher::new(
            &self.device,
            self.format.depth,
            self.shape,
            self.format.width,
            self.format.height,
            &self.planes_entries,
            picture_format(self.format.depth),
        ));
    }

    /// コーミングを数える設定か（逆テレシネが有効で、weave だけでない）。
    pub(super) fn counting(&self) -> bool {
        self.settings.telecine && self.settings.deinterlace != Deinterlace::Weave
    }

    fn count_only(&mut self, cur: usize, before: usize, first_row: u32, index: u64) {
        if !self.counting() {
            return;
        }
        // レンダリング前に飛ばしたフレームも数える（作るのを最初のレンダリングまで待つと、その間のコーミングが抜ける）。
        self.ensure_matcher();
        let Some(matcher) = self.matcher.as_mut() else {
            return;
        };
        let luma = |slot: usize| self.pool.slots[slot].views.first();
        let (Some(cur_view), Some(before_view)) = (luma(cur), luma(before)) else {
            return;
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("count"),
            });
        matcher.encode(
            &self.device,
            &self.queue,
            &mut encoder,
            cur_view,
            before_view,
            first_row,
            index,
        );
        self.queue.submit([encoder.finish()]);
        if matcher.finish(&self.device) {
            self.spent.film_changes += 1;
        }
        // 読んだスロットは GPU が読み終えるまでデコーダーに返さない（`draw` と同じ）。
        let kept: Vec<std::sync::Arc<dyn std::any::Any + Send + Sync>> = [cur, before]
            .iter()
            .filter_map(|slot| self.pool.slots[*slot].keep.clone())
            .collect();
        if !kept.is_empty() {
            self.queue.on_submitted_work_done(move || {
                drop(kept);
            });
        }
    }
}
