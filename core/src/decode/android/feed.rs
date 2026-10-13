// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Feeding the codec: one access unit, or a pair of fields, per input buffer.

use super::*;

impl Android {
    pub(super) fn submit(&mut self, tag: u64, bytes: &[u8]) -> Result<(), Error> {
        // end of stream を受けたデコーダーは flush まで何も出さない。残っている出力を取り出してからここで flush する。
        if self.ending {
            if let Some(session) = self.session.as_ref() {
                session.give(|codec| {
                    let _ = codec.flush();
                });
                if let Some(direct) = session.direct() {
                    direct.flush();
                }
            }
            self.ending = false;
            self.ended = false;
            self.fed = false;
        }
        self.step(step::INPUT, "最初の入力バッファを取る");
        let mut backed_up = false;
        let began = std::time::Instant::now();
        let patience = if self.produced { FEEDING } else { STARTING };
        for attempt in 0.. {
            if attempt >= TRIES && began.elapsed() > patience {
                break;
            }
            // 出力が詰まると入力バッファが空かないので先に取り出す（初回は待たない）。
            self.drain(if attempt == 0 { POLL } else { WAIT });
            backed_up = backed_up || self.ready.len() >= AHEAD;
            let Some(session) = self.session.as_ref() else {
                return Err(Error::Unavailable(String::from("デコーダーがありません")));
            };
            if let Some(direct) = session.direct() {
                let Some(index) = direct.input(WAIT) else {
                    continue;
                };
                direct.feed(index, bytes, tag, 0)?;
                self.fed = true;
                self.step(step::FED, "最初のアクセスユニットを渡した");
                self.drain(POLL);
                return Ok(());
            }
            let Some(codec) = session.codec.as_deref() else {
                return Err(Error::Unavailable(String::from(
                    "この経路には MediaCodec がありません",
                )));
            };
            let mut input = match codec.dequeue_input_buffer(WAIT) {
                Ok(DequeuedInputBufferResult::Buffer(input)) => input,
                Ok(DequeuedInputBufferResult::TryAgainLater) => continue,
                Err(error) => {
                    return Err(Error::Unavailable(format!(
                        "AMediaCodec_dequeueInputBuffer: {error}"
                    )));
                }
            };
            // NULL のバッファーを返すデコーダーがあり `buffer_mut` が panic するのでエラーにする。
            let copied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let room = input.buffer_mut();
                if room.len() < bytes.len() {
                    return false;
                }
                // SAFETY: `room` は `bytes` 以上で重ならず、`MaybeUninit<u8>` は `u8` と同じレイアウト。
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        room.as_mut_ptr().cast::<u8>(),
                        bytes.len(),
                    );
                }
                true
            }));
            let refused = match copied {
                Ok(true) => None,
                Ok(false) => Some(Error::Unsupported(
                    "an access unit larger than the codec's buffer",
                )),
                Err(_) => Some(Error::Unavailable(String::from(
                    "AMediaCodec_getInputBuffer が入力バッファを返しません",
                ))),
            };
            if let Some(error) = refused {
                // 取ったバッファーは空のまま返してからエラーにする。返さないと入力バッファが一つずつ減る。
                let _ = codec.queue_input_buffer(input, 0, 0, 0, 0);
                return Err(error);
            }
            // タグは表示時刻のフィールドに入れて渡す（デコード順の番号）。値はフレームごとに違うこと。
            let queued = codec.queue_input_buffer(input, 0, bytes.len(), tag, 0);
            queued.map_err(|error| {
                Error::Unavailable(format!("AMediaCodec_queueInputBuffer: {error}"))
            })?;
            self.fed = true;
            self.step(step::FED, "最初のアクセスユニットを渡した");
            self.drain(POLL);
            return Ok(());
        }
        Err(Error::Unavailable(String::from(if backed_up {
            "取り出されていないフレームが残っていて、MediaCodec の入力が空きません"
        } else {
            "MediaCodec が出力を返さないまま、入力も空きません"
        })))
    }

    pub(super) fn teardown(&mut self) {
        self.ready.clear();
        self.pending.clear();
        self.holding.clear();
        self.half = None;
        self.parity = None;
        if let Some(session) = self.session.take() {
            session.give(|codec| {
                let _ = codec.stop();
            });
            if let Some(direct) = session.direct() {
                direct.stop();
            }
        }
        self.fed = false;
        self.ending = false;
        self.ended = false;
        self.said = 0;
    }

    /// 一度に保持できるフレームの数。
    pub(super) fn room(&self) -> usize {
        if !self.ending {
            return AHEAD;
        }
        match self
            .session
            .as_ref()
            .and_then(|session| session.images.as_ref())
        {
            Some(_) => (IMAGES as usize).saturating_sub(HOLD + 1),
            None => 32,
        }
    }
}
