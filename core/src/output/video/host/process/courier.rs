// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 通知用のチャネル（`metal`・`shm`）で RUN を送るスレッド。サブミットが GPU で完了するのを待ってすぐ送る
//! （レンダリングの回で確認してから送ると、往復時間にレンダリング間隔分の誤差が乗る）。

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zwsdk::video::process as wire;

use super::STUCK;
use super::child::Writer;

pub(super) type Then = Box<dyn FnOnce() -> Result<(), String> + Send>;

pub(super) struct Errand {
    pub after: wgpu::SubmissionIndex,
    pub then: Option<Then>,
    pub run: wire::Run,
}

/// 1 回の待機の長さ（停止を要求されても気づけるように）。
const STEP: Duration = Duration::from_millis(100);

pub(super) struct Courier {
    errands: Option<mpsc::Sender<Errand>>,
    thread: Option<std::thread::JoinHandle<()>>,
    failed: Arc<Mutex<Option<String>>>,
}

impl Courier {
    pub fn new(device: wgpu::Device, writer: Writer, name: &str) -> Result<Self, String> {
        let (errands, incoming) = mpsc::channel::<Errand>();
        let failed: Arc<Mutex<Option<String>>> = Arc::default();
        let said = failed.clone();
        let thread = std::thread::Builder::new()
            .name(format!("{name} runs"))
            .spawn(move || {
                for errand in incoming {
                    if let Err(why) = deliver(&device, &writer, errand) {
                        if let Ok(mut failed) = said.lock() {
                            *failed = Some(why);
                        }
                        return;
                    }
                }
            })
            .map_err(|e| format!("スレッドを作れない：{e}"))?;
        Ok(Self {
            errands: Some(errands),
            thread: Some(thread),
            failed,
        })
    }

    pub fn send(&self, errand: Errand) -> Result<(), String> {
        self.failed()?;
        self.errands
            .as_ref()
            .and_then(|errands| errands.send(errand).ok())
            .ok_or_else(|| String::from("RUN を送るスレッドが無い"))
    }

    pub fn failed(&self) -> Result<(), String> {
        match self.failed.lock().ok().and_then(|failed| failed.clone()) {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }
}

impl Drop for Courier {
    fn drop(&mut self) {
        self.errands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn deliver(device: &wgpu::Device, writer: &Writer, errand: Errand) -> Result<(), String> {
    let began = Instant::now();
    loop {
        let waited = device.poll(wgpu::PollType::Wait {
            submission_index: Some(errand.after.clone()),
            timeout: Some(STEP),
        });
        match waited {
            Ok(_) => break,
            Err(wgpu::PollError::Timeout) if began.elapsed() < STUCK => {}
            Err(e) => return Err(format!("入力フレームのサブミットが完了しない：{e}")),
        }
    }
    if let Some(then) = errand.then {
        then()?;
    }
    writer.send(wire::RUN, &errand.run.encode())
}
