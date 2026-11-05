// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 表示側。レンダリング先、オーバーレイ、表示時刻、モニターの色。

/// 画面を映像とは別のスワップチェーンに出す（Windows、DirectComposition）。
#[cfg(target_os = "windows")]
pub mod composition;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod gles;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod layer;
pub mod monitor;
pub mod overlay;
pub mod sprites;

/// wgpu のインスタンス。別ウィンドウと共有する（別のインスタンスで作ったサーフェスは使えない）。
pub fn instance_of(backends: Option<wgpu::Backends>) -> wgpu::Instance {
    let descriptor = || {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        if let Some(backends) = backends {
            descriptor.backends = backends;
        }
        descriptor
    };
    // Windows で指定が無ければ DX12 だけ。Vulkan は初期化するだけで各社のドライバー（ICD）を読み込み、
    // 使わなくても数十 MB 残る。DX12 で描ける GPU が無いときと、Vulkan を指定したときは従来どおり。
    #[cfg(windows)]
    if backends.is_none() && std::env::var_os("WGPU_BACKEND").is_none() {
        let dx12 = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..descriptor()
        });
        if zwcore::output::video::gpu::Gpu::draws_on_hardware(&dx12) {
            return dx12;
        }
        tracing::info!("DX12 で描ける GPU が無いので、ほかのバックエンドも使う");
    }
    wgpu::Instance::new(descriptor())
}
/// egui-wgpu でレンダリングする。GPU のデバイスを失っていれば何もせず `None`。
/// egui-wgpu はデバイスを失った後に呼ぶと、ステージングバッファを作れずに panic する（`Queue::write_buffer_with` が
/// `None` を返す）。デバイスを失ったと分かるのはレンダリング中のこともあるので、そのときの panic だけここで止める。
/// デバイスを失っていないときの panic はそのまま上げる。
pub fn unless_lost<R>(paint: impl FnOnce() -> R) -> Option<R> {
    unless_lost_on(None, paint)
}

/// [`unless_lost`] の、使っているデバイスが分かっているとき（失ったかを直接問い合わせられる）。
pub fn unless_lost_on<R>(device: Option<&wgpu::Device>, paint: impl FnOnce() -> R) -> Option<R> {
    if zwcore::output::video::gpu::lost().is_some() {
        return None;
    }
    through_loss_on(device, paint)
}

/// GPU を使う処理を呼び、デバイスを失っていたときの panic だけ止めて `None`。[`unless_lost`] と違い、前に失った
/// 印が残っていても呼ぶ（開き直しの処理は、新しいデバイスを開くまで印が消えない）。
pub fn through_loss_on<R>(device: Option<&wgpu::Device>, work: impl FnOnce() -> R) -> Option<R> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(done) => Some(done),
        Err(panic) => {
            settle_panic(panic, device);
            None
        }
    }
}

/// 受け止めた panic が GPU のデバイスを失ったためなら記録して終わり、そうでなければ投げ直す。wgpu のロストの
/// 知らせは panic より遅れて届くことがあるので、デバイスが分かっていれば直接問い合わせる。
pub fn settle_panic(panic: Box<dyn std::any::Any + Send>, device: Option<&wgpu::Device>) {
    let why = zwcore::output::video::gpu::lost()
        .or_else(|| device.and_then(zwcore::output::video::gpu::removed));
    match why {
        Some(why) => tracing::warn!("GPU デバイスを失ったので GPU を使う処理を止めた（{why}）"),
        None => std::panic::resume_unwind(panic),
    }
}

pub mod scene;
pub mod shown;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod soft;
