// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! ループの一回ごとに、各スレッドが作ったものを受け取り、時計を進め、フレームを表示する。

use super::*;

/// フレームごとの表示をログに出す（`ZW_TRACE=present`）。
static SAY: crate::trace::Trace = crate::trace::Trace::of("present");

const SAY_MOST: u32 = 6000;

fn say(what: &str, gap: f32, lag: f32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SHOWN: AtomicU32 = AtomicU32::new(0);
    if !SAY.on() {
        return;
    }
    let at = SHOWN.fetch_add(1, Ordering::Relaxed);
    if at >= SAY_MOST {
        return;
    }
    tracing::info!(
        "{} ms  表示 {at} {what} 間隔 {gap:.1} ms 遅れ {lag:.1} ms",
        crate::trace::since_start(),
    );
}

impl Player {
    /// Stopping here is what stops the threads. Says whether it took anything.
    pub fn feed(&mut self, painter: &Painter) -> bool {
        #[cfg(all(feature = "wgpu", feature = "plugins"))]
        self.remember_host(painter);
        let began = Instant::now();
        let before = self.took;
        self.take_all(painter);
        self.meter.feed += began.elapsed();
        self.took != before
    }

    fn take_all(&mut self, painter: &Painter) {
        // 音声はここを通らない（サーフェスの待ち時間で音声が途切れないようにするため）。
        let mut pictures_open = self.pictures.is_some();
        while pictures_open && self.ready.len() < AHEAD {
            let Some(pictures) = self.pictures.as_ref() else {
                break;
            };
            match pictures.try_recv() {
                Ok(out) => self.take(out, painter),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pictures = None;
                    pictures_open = false;
                }
            }
            self.pump();
        }
        self.done =
            !self.holding && self.pictures.is_none() && self.shared.ended.load(Ordering::Relaxed);
        self.pump();
    }

    fn take(&mut self, out: Out, painter: &Painter) {
        self.took += 1;
        match out {
            Out::Picture { .. } if self.shared.viewer_off.load(Ordering::Relaxed) => {}
            Out::Picture { at, picture } => {
                self.first_picture.get_or_insert_with(Instant::now);
                let about = picture.about();
                if self.video.is_none() || self.format != Some(about.format) {
                    self.format = Some(about.format);
                    self.tell_speed();
                    self.settings = painter.settings(about.format);
                    let deinterlaced = self
                        .spent_decoding
                        .about
                        .lock()
                        .is_ok_and(|decoder| decoder.deinterlaced);
                    if deinterlaced {
                        self.settings.deinterlace = Deinterlace::Weave;
                        self.settings.telecine = false;
                    }
                    if let Some(step) = self.options.deinterlace.filter(|_| !deinterlaced) {
                        self.settings.deinterlace = step;
                        self.settings.deinterlace_chosen = true;
                        if step == Deinterlace::Weave {
                            self.settings.telecine = false;
                        }
                    }
                    if let Some(telecine) = self.options.telecine {
                        self.settings.telecine = telecine && self.settings.telecine;
                    }
                    // 要求されたがデコーダー側でデインターレースできなかった。ログを出してからシェーダーに切り替える。
                    if self.options.deinterlace_by == DeinterlaceBy::Hardware
                        && !deinterlaced
                        && !about.progressive
                        && !self.said_no_hardware
                    {
                        tracing::info!(
                            "  ハードウェアのデインターレースが使えない。シェーダーで処理する（--deinterlace-by software 相当）"
                        );
                        self.said_no_hardware = true;
                    }
                    let how = if deinterlaced {
                        String::from("デコーダーでデインターレース済み")
                    } else if about.format.field_pictures && about.structure.field() {
                        // 1枚が1フィールドのまま届いた（GLES 2.0 が外部テクスチャで描く。[`crate::decode::takes::takes_lone_fields`]）。
                        String::from(
                            "フィールド構造ピクチャーを1フィールドずつ半行ずらして表示（bob 相当）",
                        )
                    } else if self.settings.deinterlace == Deinterlace::Weave {
                        if self.options.deinterlace == Some(Deinterlace::Weave) {
                            String::from("指定によりデインターレースなし")
                        } else if about.format.field_pictures {
                            // 1枚が1フィールドのストリーム（HEVC の `field_seq_flag`）。
                            let (num, den) = about.format.aspect;
                            format!("フィールド構造ピクチャー、{num}:{den} に伸ばして表示")
                        } else {
                            String::from("プログレッシブ")
                        }
                    } else {
                        let telecine = if self.settings.telecine {
                            "、逆テレシネ"
                        } else {
                            ""
                        };
                        let woven = if about.format.woven {
                            "フィールド構造ピクチャーを組んで"
                        } else {
                            ""
                        };
                        format!(
                            "{woven}デインターレース {}{telecine}",
                            self.settings.deinterlace.name()
                        )
                    };
                    tracing::info!(
                        "映像 {}x{} {}bit  {how}",
                        about.format.width,
                        about.format.height,
                        about.format.depth
                    );
                    let tune = self.options.deinterlace.is_none() && !deinterlaced;
                    // 古い映像出力を先に破棄する（GLES では古い映像出力の後片付けが新しいコンテキストの物を消す）。
                    self.video = None;
                    match painter.build(about.format, self.settings, tune) {
                        Ok(video) => {
                            // 要求した処理が組み込まれたとは限らない（GLES 2.0 では外れる）。
                            let built = video.settings();
                            if built.deinterlace != self.settings.deinterlace
                                || built.telecine != self.settings.telecine
                            {
                                tracing::info!(
                                    "  計測により {} を使用{}{}",
                                    built.deinterlace.name(),
                                    if built.telecine {
                                        "、逆テレシネ"
                                    } else {
                                        "、逆テレシネは使えないためオフ"
                                    },
                                    video.about(),
                                );
                            }
                            self.settings = built;
                            let mut video = video;
                            video.set_inset(self.inset);
                            video.set_pan_scan(self.pan_scan);
                            video.set_scaling(self.scaling());
                            video.set_display_rate(self.display_mhz);
                            self.video = Some(video);
                        }
                        Err(error) => tracing::warn!("映像の出力を準備できない: {error:?}"),
                    }
                }
                if self.video.is_none() {
                    return;
                }
                self.ready.push_back((at, picture));
            }
            Out::Sound { .. } | Out::Coded { .. } => {
                tracing::warn!("音声が映像のチャネルに届いた");
            }
            // 番組が切り替わった。前の番組のフレームは新しい時計では表示時刻にならないので捨てる。
            Out::Switched => {
                self.ready.clear();
                self.due.clear();
                self.first_picture = None;
                // 組み立て途中のフレームも捨てる（でないと前の番組のフレームに新しい時刻が付く）。
                if let Some(video) = self.video.as_mut() {
                    video.reset();
                }
                self.drew = false;
                self.stalled = false;
            }
            Out::Trouble { stream, what } => {
                tracing::warn!("ストリーム 0x{stream:04x}: {what}");
            }
            Out::DataStreams(_)
            | Out::ApplicationService(_)
            | Out::Data { .. }
            | Out::Item { .. }
            | Out::CaptionStreams(_)
            | Out::Caption { .. } => {}
            Out::Access(unit) => {
                tracing::warn!(
                    "ストリーム 0x{:04x}: デコードされていない映像が届いた",
                    unit.stream
                );
            }
        }
    }

    fn finish(&mut self) {
        if !self.done || self.flushed || !self.ready.is_empty() {
            return;
        }
        self.flushed = true;
        if let Some(video) = self.video.as_mut() {
            video.flush();
        }
    }

    fn poll_video(&self) {
        if let Some(video) = self.video.as_ref() {
            video.poll();
        }
    }

    /// Counted as due only once accepted.
    fn pump(&mut self) {
        if let Some(tables) = self.tables.as_ref()
            && let Some(last) = tables.try_iter().last()
        {
            self.last_tables = last;
            self.told += 1;
            if self.audio_only() && (self.video.is_some() || self.format.is_some()) {
                self.video = None;
                self.format = None;
                self.ready.clear();
                self.due.clear();
            }
        }
        let now = self.now();
        if self.hidden.load(Ordering::Relaxed) {
            return;
        }
        let Some(video) = self.video.as_mut() else {
            return;
        };
        while let Some((at, frame)) = self.ready.front() {
            // 表示時刻を過ぎたフレームは渡さない（解像度が高いと転送がさらに遅れを生む悪循環になる）。最後の一枚は渡す。
            if let Some(now) = now
                && self.ready.len() > 1
                && *at < now
            {
                let about = frame.about();
                if at + frame_ticks(about.format) < now {
                    self.ready.pop_front();
                    self.late += 1;
                    self.drew = true;
                    continue;
                }
            }
            // 出力の数は渡した先に問い合わせる（逆テレシネで数え直すとずれる）。
            // 時刻そのものは割らない（ソフトテレシネで番号が前後する）。
            let ticks = frame_ticks(frame.about().format);
            let number = match self.numbered {
                Some((was, number)) if *at > was && *at - was < HZ => {
                    number + ((*at - was + ticks / 2) / ticks).max(1) as u64
                }
                Some((_, number)) => number + 1,
                None => 0,
            };
            let outputs = match video.push_at(frame, number) {
                Ok(outputs) => outputs,
                Err(error) => {
                    if !matches!(error, VideoError::Full) && !self.refused {
                        self.refused = true;
                        tracing::warn!("映像を GPU に渡せない: {error:?}");
                    }
                    break;
                }
            };
            self.numbered = Some((*at, number));
            // フレーム補間では位相に応じた時刻にする（等間隔ではない）。
            let about = frame.about();
            let ticks = frame_ticks(about.format);
            let phases = video.last_phases();
            let step = if outputs == 0 {
                0
            } else {
                ticks / outputs as i64
            };
            for which in 0..outputs {
                let offset = match phases.get(which) {
                    Some(phase) if phases.len() == outputs => {
                        (f64::from(*phase) * ticks as f64) as i64
                    }
                    _ => which as i64 * step,
                };
                self.due.push_back(Due { at: at + offset });
            }
            self.ready.pop_front();
        }
        #[cfg(feature = "plugins")]
        {
            let held = self.video.as_ref().map_or(0.0, Screen::filter_held_ms);
            self.set_video_held(held);
        }
    }

    /// Asked before the surface is touched: a texture taken and not presented uses up a DXGI
    /// signal, and three of those stall the next take for a second.
    pub fn wants_turn(&mut self, elapsed: i64) -> bool {
        self.settle(elapsed);
        if self.done && self.ready.is_empty() && self.due.is_empty() {
            return true;
        }
        let Some(now) = self.now() else {
            return false;
        };
        self.due.front().is_some_and(|due| due.at <= now)
    }

    /// 音声の無い番組か。そのときは時計を実時間で進め、音声が後から来れば音声の時計に引き継ぐ。
    fn soundless(&self) -> bool {
        self.last_tables.video.is_some() && self.last_tables.sound.is_none()
    }

    fn move_clock(&mut self, elapsed: i64) {
        self.shared
            .picture
            .store(!self.due.is_empty(), Ordering::Relaxed);
        if self.has_sound && !self.soundless() {
            return;
        }
        if self.paused || self.buffering {
            return;
        }
        // ライブ再生では、デコードできた一番新しいフレームの時刻より先へ進めない。でないと受信やデコードが
        // 一瞬遅れただけで、遅れたフレームがまとめて表示時刻を過ぎてしまう。
        let latest = if self.live_source {
            let latest = self
                .due
                .back()
                .map(|due| due.at)
                .into_iter()
                .chain(self.ready.back().map(|(at, _)| *at))
                .max();
            if latest.is_none() && self.shared.with_clock(|clock| clock.now().is_some()) {
                return;
            }
            latest
        } else {
            None
        };
        let elapsed = (elapsed as f64 * self.told_speed) as i64;
        // Anchored once. Anchoring every turn draws one picture per turn however fast they come.
        let first = self.due.front().map(|due| due.at);
        // ライブ再生では、最初のフレームから表示できるまでの間も時計を進め、溜まったフレームは読み飛ばす。
        // でないと溜まった分ずっと遅れ、符号化データのチャネルが溢れ続ける。
        let waited = match self.first_picture {
            Some(at) if self.live_source => (at.elapsed().as_secs_f64() * HZ as f64) as i64,
            _ => 0,
        };
        self.shared.with_clock(|clock| {
            if clock.now().is_none()
                && let Some(at) = first
            {
                clock.start(at + waited);
            }
            // Never by more than a few frames at once (Windows holds this loop during a drag).
            let mut step = elapsed.min(MOST_ADVANCE);
            if let (Some(latest), Some(now)) = (latest, clock.now()) {
                step = step.min((latest - now).max(0));
            }
            clock.advance(step);
        });
    }

    /// レンダリングせずにループを一回だけ回す（ウィンドウが隠れているとき）。時計は進め、表示時刻を過ぎたフレームは捨てる。
    pub fn idle(&mut self, elapsed: i64) -> Tick {
        self.away.called(Instant::now(), false);
        self.settle(elapsed);
        let mut tick = self.fresh_tick();
        let Some(now) = self.now() else {
            return tick;
        };
        if self.hidden.load(Ordering::Relaxed) {
            while self.ready.front().is_some_and(|(at, _)| *at < now) {
                self.ready.pop_front();
                self.unseen += 1;
                tick.did = true;
            }
        }
        while self.due.front().is_some_and(|due| due.at <= now) {
            // 捨てるのは対で（でないと時刻が一つ先の出力に付く）。
            let skipped = self.video.as_mut().is_some_and(|video| video.skip());
            if self.video.is_some() && !skipped {
                break;
            }
            self.due.pop_front();
            self.late += 1;
            tick.did = true;
            self.drew = true;
            self.pump();
        }
        self.watch_first();
        self.watch_back();
        self.measure();
        tick
    }

    pub fn tick(&mut self, canvas: &Canvas<'_>, elapsed: i64) -> Tick {
        let began = Instant::now();
        self.away.called(began, true);
        let tick = self.turn(canvas, elapsed);
        self.meter.tick += began.elapsed();
        self.meter.turns += 1;
        self.watch_first();
        self.watch_back();
        self.measure();
        tick
    }

    /// 隠れた。表示待ちと GPU に渡した出力を捨てる（残すと戻った途端に読み飛ばしが起きる）。
    pub(super) fn go_away(&mut self) {
        self.away.hide(Instant::now());
        let thrown = self.throw_due();
        self.unseen += thrown as u64;
    }

    /// 戻った。古くなった物を捨て、現在の時刻から表示し直す（[`BACK_BEHIND`]）。
    pub(super) fn come_back(&mut self) {
        let counts = self.away_counts();
        if !self.away.show(Instant::now(), counts) {
            return;
        }
        self.spent_reading.forget_longest();
        let due = self.throw_due();
        let mut ready = 0;
        if let Some(now) = self.now() {
            // 最後の一枚は残す（画面が空になるほうが目立つ）。
            while self.ready.len() > 1
                && self
                    .ready
                    .front()
                    .is_some_and(|(at, frame)| at + frame_ticks(frame.about().format) < now)
            {
                self.ready.pop_front();
                ready += 1;
            }
            let decoding = self.oldest_access.load(Ordering::Relaxed) as i64;
            if back_behind(decoding, now) {
                self.skip_to.fetch_max(now, Ordering::Relaxed);
            }
        }
        self.unseen += ready as u64;
        self.away.threw(due, ready);
        self.since = 0;
        self.stalled = false;
    }

    fn throw_due(&mut self) -> usize {
        let thrown = self.due.len();
        self.due.clear();
        if let Some(video) = self.video.as_mut() {
            // 返り値でループを抜ける（`pending` だけを見ると抜けられないことがある）。
            while video.pending() > 0 && video.skip() {}
        }
        thrown
    }

    fn away_counts(&self) -> away::Counts {
        away::Counts {
            drawn: self.drawn,
            late: self.late,
            dropped: self.dropped_access.load(Ordering::Relaxed),
            missed_blocks: self.spent_reading.missed.load(Ordering::Relaxed),
            holes: self.shared.missed.load(Ordering::Relaxed),
            left: self.left,
            longest: self.spent_reading.longest(),
        }
    }

    fn watch_back(&mut self) {
        let counts = self.away_counts();
        if let Some(said) = self.away.report(Instant::now(), counts) {
            tracing::info!("{said}");
        }
    }

    fn turn(&mut self, canvas: &Canvas<'_>, elapsed: i64) -> Tick {
        self.poll_video();
        self.watch_reading();
        self.settle(elapsed);
        let mut tick = self.fresh_tick();
        let Some(now) = self.now() else {
            return tick;
        };
        self.leave_behind(now);
        // レンダリングせずに戻ると数枚前のフレームが残る。
        let mut drew_now = false;
        while self.due.front().is_some_and(|due| due.at <= now) {
            let Some(video) = self.video.as_mut() else {
                self.due.pop_front();
                continue;
            };
            let lost = video.take_lost();
            if lost > 0 {
                let lost = lost.min(self.due.len());
                self.due.drain(..lost);
                continue;
            }
            if !video.ready() {
                break;
            }
            // Something after this one is due, so it is thrown away (drawing late stays late).
            let late = self.due.iter().take_while(|due| due.at <= now).count();
            if late > self.pile_most {
                // 時刻の列は出力より一つ長いので、飛ばせた出力の分だけ捨てる。
                for _ in 0..late - 1 {
                    if !video.skip() {
                        break;
                    }
                    self.due.pop_front();
                    self.late += 1;
                    say("スキップ", 0.0, 0.0);
                }
                // The timeline is moving; the watchdog must not call this a stop.
                self.drew = true;
                tick.did = true;
                continue;
            }
            let behind = self
                .due
                .get(1)
                .is_some_and(|next| next.at + BEHIND_SLACK <= now);
            if behind && video.pending() > 1 && video.skip() {
                self.due.pop_front();
                self.late += 1;
                say("スキップ", 0.0, 0.0);
                tick.did = true;
                self.drew = true;
                continue;
            }
            let moment = self.due.front().map_or(now, |due| due.at);
            match video.draw(canvas) {
                Ok(true) => {
                    drew_now = true;
                    self.away.drew(Instant::now());
                    self.due.pop_front();
                    self.drawn += 1;
                    self.drew = true;
                    tick.did = true;
                    let at = Instant::now();
                    let gap = self.drew_at.map_or(0.0, |before| {
                        at.duration_since(before).as_secs_f32() * 1000.0
                    });
                    if self.drew_at.is_some() {
                        self.meter.gaps.push(gap);
                    }
                    self.drew_at = Some(at);
                    let lag = (now - moment) as f32 * 1000.0 / HZ as f32;
                    self.meter.lags.push(lag);
                    say("表示", gap, lag);
                }
                Ok(false) => break,
                // A surface goes stale on resize or display change; set up again.
                Err(VideoError::Surface(_)) => {
                    tick.reconfigure = true;
                    break;
                }
                Err(error) => {
                    tracing::warn!("映像を表示できない: {error:?}");
                    break;
                }
            }
            // サーフェスはループ一回につき一枚。続けてレンダリングすると現在のフレームが上書きされて表示されない。
            self.pump();
            break;
        }
        // 何もレンダリングしなかったら前のフレームをもう一度表示する（でないと数枚前のフレームが挟まる）。
        if !drew_now && let Some(video) = self.video.as_mut() {
            video.redraw(canvas);
            self.redrawn += 1;
            say("再描画", 0.0, 0.0);
        }
        self.watch(elapsed);
        tick.more = self.more();
        tick
    }

    fn settle(&mut self, elapsed: i64) {
        self.pump();
        self.finish();
        self.move_clock(elapsed);
    }

    fn more(&self) -> bool {
        !self.done || !self.ready.is_empty() || !self.due.is_empty()
    }

    fn fresh_tick(&self) -> Tick {
        Tick {
            more: self.more(),
            reconfigure: false,
            did: false,
        }
    }

    /// 映像が追いつけないほど遅れていたら、現在位置まで読み飛ばす。
    fn leave_behind(&mut self, now: i64) {
        let Some(head) = self.due.front().map(|due| due.at) else {
            return;
        };
        let most = if self.tuned.elapsed() < SETTLING {
            SETTLING_BEHIND
        } else {
            BEHIND_MOST
        };
        if now - head <= most {
            return;
        }
        // 飛ばした直後は測らない（チャネルの中身はまだ古い）。
        if self
            .left_at
            .is_some_and(|at: Instant| at.elapsed() < BEHIND_APART)
        {
            return;
        }
        self.left_at = Some(Instant::now());
        self.left += 1;
        tracing::info!(
            "映像が {:.1} 秒遅れたため、現在位置まで飛ばす",
            (now - head) as f64 / HZ as f64
        );
        self.skip_to.fetch_max(now, Ordering::Relaxed);
        let left = self.ready.len() + self.due.len();
        self.ready.clear();
        self.due.clear();
        if let Some(video) = self.video.as_mut() {
            // 返り値でループを抜ける（`pending` だけを見ると抜けられないことがある）。
            while video.pending() > 0 && video.skip() {}
        }
        self.late += left as u64;
        self.drew = true;
    }

    fn watch(&mut self, elapsed: i64) {
        if self.drew {
            self.since = 0;
            self.drew = false;
            self.stalled = false;
            return;
        }
        if self.ready.is_empty() && self.due.is_empty() {
            return;
        }
        self.since += elapsed;
        if self.stalled || self.since < HZ {
            return;
        }
        self.stalled = true;
        tracing::info!(
            "映像が 1 秒止まっている: デコード済み {} 枚、表示待ち {} 個、GPU {} 枚、音声の残り {} サンプル、時刻 {:?}",
            self.ready.len(),
            self.due.len(),
            self.video.as_ref().map_or(0, Screen::pending),
            self.shared.queued.load(Ordering::Relaxed),
            self.now(),
        );
    }
}

/// 戻ったときデコードがこれだけ遅れていたら、古い符号化データを次のランダムアクセスポイントまで読み飛ばす。
const BACK_BEHIND: i64 = HZ / 2;

fn back_behind(decoding: i64, now: i64) -> bool {
    decoding > 0 && now - decoding > BACK_BEHIND
}

#[cfg(test)]
mod tests {
    use super::{BACK_BEHIND, Due, Player, back_behind};
    use crate::engine::clock::HZ;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    fn quiet_player() -> Player {
        Player::from_source(
            crate::input::source::pending(),
            crate::filters::Container::Ts,
            None,
            super::super::Options::default(),
            None,
        )
    }

    #[test]
    fn hiding_for_two_minutes_and_coming_back_leaves_nothing_stale() {
        let mut player = quiet_player();
        let start = 10 * HZ;
        player.shared.with_clock(|clock| clock.start(start));
        for n in 0..5 {
            player.due.push_back(Due {
                at: start + n * 3003,
            });
        }
        player.oldest_access.store(start as u64, Ordering::Relaxed);
        player.set_hidden(true);
        assert!(player.due.is_empty(), "隠れたら表示待ちは残さない");
        assert_eq!(player.unseen, 5);
        for _ in 0..1200 {
            player.idle(HZ / 10);
        }
        let now = player.now().expect("時計は動いている");
        assert!(now >= start + 119 * HZ, "{now}");
        player.set_hidden(true);
        assert_eq!(player.skip_to.load(Ordering::Relaxed), 0);
        player.set_hidden(false);
        assert_eq!(
            player.skip_to.load(Ordering::Relaxed),
            now,
            "隠れた時刻のままの符号化データは、現在の時刻まで読み飛ばす"
        );
        assert!(player.due.is_empty());
        let counts = player.away_counts();
        let said = player
            .away
            .report(
                Instant::now() + super::away::BACK_WATCH + Duration::from_millis(1),
                counts,
            )
            .expect("戻ってからの一行");
        assert!(said.contains("ウィンドウが戻った"), "{said}");
        assert!(said.contains("表示待ち 0"), "{said}");
    }

    #[test]
    fn coming_back_with_the_decoder_ahead_skips_nothing() {
        let mut player = quiet_player();
        let start = 10 * HZ;
        player.shared.with_clock(|clock| clock.start(start));
        player.set_hidden(true);
        for _ in 0..50 {
            player.idle(HZ / 10);
        }
        let now = player.now().expect("時計は動いている");
        player
            .oldest_access
            .store((now + HZ / 3) as u64, Ordering::Relaxed);
        player.set_hidden(false);
        assert_eq!(player.skip_to.load(Ordering::Relaxed), 0);
        player.set_hidden(false);
        assert_eq!(player.skip_to.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_buffer_and_the_queue_follow_the_settings() {
        let mut player = quiet_player();
        player.set_buffer(Duration::from_secs(5));
        assert_eq!(player.hold.load(Ordering::Relaxed), 5_000);
        player.set_buffer(Duration::ZERO);
        assert_eq!(
            player.hold.load(Ordering::Relaxed),
            1,
            "持たないバッファにはしない"
        );
        player.set_queue(6);
        assert_eq!(player.pile_most, 6);
        player.set_queue(0);
        assert_eq!(player.pile_most, 1);
    }

    #[test]
    fn a_decoder_that_kept_up_is_not_skipped() {
        let now = 120 * HZ;
        assert!(!back_behind(now + HZ / 4, now), "先までデコードしている");
        assert!(
            !back_behind(now - HZ / 10, now),
            "数枚の遅れならランダムアクセスポイントを待つより良い"
        );
        assert!(!back_behind(now - BACK_BEHIND, now));
        assert!(!back_behind(0, now), "まだ何も取っていない");
    }

    #[test]
    fn a_decoder_left_behind_skips_to_now() {
        let now = 120 * HZ;
        assert!(back_behind(now - BACK_BEHIND - 1, now));
        assert!(back_behind(now - 60 * HZ, now));
    }
}
