// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Draining the codec: pictures out, onto the window or read back.

use super::*;

impl Android {
    /// 保持できる所までデコーダーから取り出す。回数と時間で区切り、超えたら [`Android::spun`] に数える。
    pub(super) fn drain(&mut self, patience: std::time::Duration) {
        let began = std::time::Instant::now();
        let mut turns = 0usize;
        while self.ready.len() < self.room() {
            turns += 1;
            if turns > TURNS || began.elapsed() > DRAINING {
                self.spun += 1;
                return;
            }
            if !self.drain_one(patience) {
                return;
            }
        }
    }

    /// end of stream の後、デコーダーが終端フラグを出すまで取り出す。時間は上限に過ぎない。
    pub(super) fn drain_ending(&mut self) {
        if !self.ending || self.ended {
            return;
        }
        let began = std::time::Instant::now();
        while !self.ended && self.ready.len() < self.room() {
            if began.elapsed() > ENDING {
                self.spun += 1;
                self.step(step::LATE, "終端を待つ間にタイムアウトしました");
                return;
            }
            self.drain_one(ENDING_WAIT);
        }
    }

    /// 一枚取る。無ければ `false`。
    pub(super) fn drain_one(&mut self, patience: std::time::Duration) -> bool {
        // AImageReader に出したフレームを先に取る。render した後はデコーダーが二度と通知しないので、取らないと失う。
        if let Some(tag) = self.pending.front().copied() {
            return match self.acquire(tag) {
                Taken::Yes => {
                    self.pending.pop_front();
                    true
                }
                Taken::Full => false,
                Taken::Late => {
                    // 永遠には待たず捨てて `Android::missed` に数える。
                    self.missed += 1;
                    self.pending.pop_front();
                    true
                }
            };
        }
        self.step(step::OUTPUT, "最初の出力バッファを待つ");
        let said = {
            let Some(session) = self.session.as_ref() else {
                return false;
            };
            if let Some(direct) = session.direct() {
                let Some((index, flags, when)) = direct.output(patience) else {
                    return false;
                };
                if flags & media::FLAG_END_OF_STREAM != 0 {
                    drop(direct.hold(index));
                    Said::Ended
                } else {
                    Said::Screen(direct.hold(index), when.max(0) as u64)
                }
            } else {
                let Some(codec) = session.codec.as_deref() else {
                    return false;
                };
                match codec.dequeue_output_buffer(patience) {
                    Ok(DequeuedOutputBufferInfoResult::Buffer(buffer)) => {
                        let info = *buffer.info();
                        let tag = info.presentation_time_us().max(0) as u64;
                        if info.flags() & media::FLAG_END_OF_STREAM != 0 {
                            let _ = codec.release_output_buffer(buffer, false);
                            Said::Ended
                        } else if info.size() <= 0 {
                            let _ = codec.release_output_buffer(buffer, false);
                            Said::Again
                        } else if session.route == Route::Buffers {
                            match codec.release_output_buffer(buffer, true) {
                                Ok(()) => Said::Acquire(tag),
                                Err(_) => Said::Nothing,
                            }
                        } else {
                            let shape = codec.output_format();
                            let layout = media::layout(&shape);
                            let read =
                                media::planes(&buffer, &shape, tag, session.format, self.weave);
                            let _ = codec.release_output_buffer(buffer, false);
                            match read {
                                Ok(frame) => Said::Frame(Box::new(frame), layout),
                                Err(why) => Said::Unreadable(why, layout),
                            }
                        }
                    }
                    // 通常の戻り値で、止まれの意味。「もう一度」にすると待ち時間 0 で永遠にループする。
                    Ok(DequeuedOutputBufferInfoResult::TryAgainLater) => Said::Nothing,
                    Ok(_) => Said::Again,
                    Err(_) => Said::Nothing,
                }
            }
        };
        match said {
            Said::Nothing => false,
            Said::Ended => {
                self.step(step::ENDED, "終端フラグを受け取りました");
                self.ended = true;
                false
            }
            Said::Again => true,
            Said::Acquire(tag) => {
                self.step(step::GOT, "最初の出力バッファが来た");
                self.step(step::RENDER, "releaseOutputBuffer(render=true) を出した");
                self.produced = true;
                self.pending.push_back(tag);
                true
            }
            Said::Screen(screen, tag) => {
                self.step(step::GOT, "最初の出力バッファが来た");
                self.step(
                    step::PICTURE,
                    "最初のフレームが出た（直接出力。インデックスだけ）",
                );
                self.produced = true;
                let format = self
                    .session
                    .as_ref()
                    .map_or(Format::default(), |session| session.format);
                let about = About {
                    tag,
                    format,
                    structure: Structure::Frame,
                    top_field_first: true,
                    repeat_first_field: false,
                    progressive: format.progressive,
                };
                self.ready.push_back(Decoded::Surface(Surface {
                    about,
                    held: Where::Screen(screen),
                }));
                true
            }
            Said::Frame(frame, layout) => {
                self.step(step::GOT, "最初の出力バッファが来た");
                self.step(step::LAYOUT, &format!("出力のレイアウト {layout}"));
                self.step(step::PICTURE, "最初のフレームが出た（メモリ）");
                self.produced = true;
                self.ready.push_back(Decoded::Planes(*frame));
                true
            }
            Said::Unreadable(why, layout) => {
                self.step(step::GOT, "最初の出力バッファが来た");
                self.step(step::LAYOUT, &format!("出力のレイアウト {layout}"));
                if self.unreadable == 0 {
                    self.step(step::UNREADABLE, &format!("読めません: {why}"));
                }
                self.produced = true;
                self.unreadable += 1;
                self.why = Some(why.to_string());
                true
            }
        }
    }

    /// AImageReader から一枚取る。`MaxImagesAcquired` は待たない（呼ぶ側が取るまで変わらない）。
    pub(super) fn acquire(&mut self, tag: u64) -> Taken {
        self.step(step::IMAGE, "AImageReader から AImage を取る");
        let Some(session) = &self.session else {
            return Taken::Full;
        };
        let Some(images) = &session.images else {
            return Taken::Full;
        };
        let format = session.format;
        let began = std::time::Instant::now();
        let image = loop {
            match images.acquire() {
                media::Got::Image(image) => break image,
                media::Got::Full => return Taken::Full,
                media::Got::None => {
                    if began.elapsed() >= LATENCY {
                        let why = format!(
                            "AImageReader_acquireNextImage が {} ms 以内にフレームを返しません（タイムアウト）",
                            LATENCY.as_millis()
                        );
                        self.step(step::ACQUIRE_LATE, &why);
                        self.why = Some(why);
                        return Taken::Late;
                    }
                    // ビジーループせずスリープする。小さな CPU ではループするとデコーダーの CPU 時間を奪う。
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                media::Got::Failed(status) => {
                    let why = media::status("AImageReader_acquireNextImage", status);
                    self.step(step::ACQUIRE_FAILED, &why);
                    self.why = Some(why);
                    return Taken::Late;
                }
            }
        };
        let image = Arc::new(image);
        // 表示中のフレームを上書きされないよう保持しておく。短さの理由は [`IMAGES`] を見よ。
        self.holding.push_back(Arc::clone(&image));
        while self.holding.len() > HOLD {
            self.holding.pop_front();
        }
        let about = About {
            tag,
            format,
            structure: Structure::Frame,
            top_field_first: true,
            repeat_first_field: false,
            progressive: format.progressive,
        };
        self.ready.push_back(Decoded::Surface(Surface {
            about,
            held: Where::HardwareBuffer(Frame::new(image, format)),
        }));
        self.step(step::PICTURE, "最初のフレームが出た（AHardwareBuffer）");
        Taken::Yes
    }

    /// 同じフレームをメモリーに読み戻す（ソフトウェアデコーダーと比べるためだけ）。
    pub(super) fn readback(&mut self) -> Option<Readback> {
        // 読めないフレームは飛ばして数える。最初で `None` を返すと何も出さないデコーダーに見える。
        while let Some(found) = self.ready.pop_front() {
            let about = found.about();
            match found {
                Decoded::Planes(frame) => {
                    return Some(Readback {
                        about,
                        planes: frame
                            .planes
                            .iter()
                            .map(|plane| (plane.data.clone(), plane.stride, plane.height))
                            .collect(),
                    });
                }
                Decoded::Surface(surface) => {
                    if let Where::Screen(_) = &surface.held {
                        self.unreadable += 1;
                        self.why = Some(String::from(
                            "直接出力の経路にはピクセルが来ません（インデックスだけです）",
                        ));
                        continue;
                    }
                    let Where::HardwareBuffer(frame) = &surface.held else {
                        self.unreadable += 1;
                        self.why = Some(String::from("このビルドが対応していない形式のフレーム"));
                        continue;
                    };
                    match frame.read() {
                        Ok(planes) => return Some(Readback { about, planes }),
                        Err(why) => {
                            if self.unreadable == 0 {
                                self.step(step::UNREADABLE, &format!("読み戻せません: {why}"));
                            }
                            self.unreadable += 1;
                            self.why = Some(why.to_string());
                        }
                    }
                }
            }
        }
        None
    }
}
