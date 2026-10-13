// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 画面（egui）を映像とは別のスワップチェーンに出す（Windows、DirectComposition）。
//! 既定では使わない（`ZW_OVERLAY=1`）。高リフレッシュか HDR の画面では映像が DWM の合成に落ちる。

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::core::Interface;

use crate::draw::overlay::{Drawn, Mixer};

pub struct Overlay {
    device: IDCompositionDevice,
    /// 保持していないとビジュアルがウィンドウから外れる。
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    surface: wgpu::Surface<'static>,
    size: (u32, u32),
    configured: bool,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Overlay")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

// SAFETY: DirectComposition の COM オブジェクトは free-threaded で、同時に触ることは無い。
unsafe impl Send for Overlay {}

impl Overlay {
    /// ウィンドウにコンポジションターゲットを作り、ビジュアルを一つ置いて、そこからサーフェスを作る。ウィンドウを作ったスレッドで呼ぶ。
    pub fn open(instance: &wgpu::Instance, hwnd: isize) -> Result<Self, String> {
        // SAFETY: レンダリングデバイスは要らないので `None`。
        let device: IDCompositionDevice = unsafe { DCompositionCreateDevice2(None) }
            .map_err(|error| format!("DCompositionCreateDevice2: {error}"))?;
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: 有効なウィンドウの HWND。
        let target = unsafe { device.CreateTargetForHwnd(hwnd, true) }
            .map_err(|error| format!("CreateTargetForHwnd: {error}"))?;
        let visual =
            unsafe { device.CreateVisual() }.map_err(|error| format!("CreateVisual: {error}"))?;
        unsafe { target.SetRoot(&visual) }.map_err(|error| format!("SetRoot: {error}"))?;
        unsafe { device.Commit() }.map_err(|error| format!("Commit: {error}"))?;
        // SAFETY: 生きている `IDCompositionVisual`。wgpu は参照を一つ増やして持つ。
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                visual.as_raw(),
            ))
        }
        .map_err(|error| format!("ビジュアルからサーフェスが作れない: {error}"))?;
        Ok(Self {
            device,
            _target: target,
            _visual: visual,
            surface,
            size: (0, 0),
            configured: false,
        })
    }

    pub fn configure(&mut self, adapter: &wgpu::Adapter, device: &wgpu::Device, size: (u32, u32)) {
        if self.configured && self.size == size {
            return;
        }
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let capabilities = self.surface.get_capabilities(adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| *format == FORMAT)
            .unwrap_or_else(|| capabilities.formats.first().copied().unwrap_or(FORMAT));
        let present_mode = [
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Fifo,
        ]
        .into_iter()
        .find(|mode| capabilities.present_modes.contains(mode))
        .unwrap_or(wgpu::PresentMode::Fifo);
        let alpha_mode = if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        self.surface.configure(
            device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: size.0,
                height: size.1,
                present_mode,
                desired_maximum_frame_latency: 2,
                alpha_mode,
                color_space: wgpu::SurfaceColorSpace::Auto,
                view_formats: Vec::new(),
            },
        );
        // wgpu はビジュアルに設定したが `Commit` はしないので、ここで効かせる。
        if let Err(error) = unsafe { self.device.Commit() } {
            tracing::warn!("画面の合成: Commit: {error}");
        }
        self.size = size;
        self.configured = true;
    }

    pub fn frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        if !self.configured {
            return None;
        }
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => None,
            other => {
                tracing::warn!("画面のサーフェスを取得できない: {other:?}");
                self.configured = false;
                None
            }
        }
    }
}

/// 重ねるサーフェスはガンマ空間の形式。シートの値をそのまま書き、OS がガンマの空間で重ねる（`Sheet::gamma`）。
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// 画面のスワップチェーンに出すスレッドを起動し、バッチを渡すチャネルを返す。
pub fn spawn(
    mut overlay: Overlay,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
) -> Option<std::sync::mpsc::Sender<Box<Drawn>>> {
    let (send, receive) = std::sync::mpsc::channel::<Box<Drawn>>();
    let spawned = std::thread::Builder::new()
        .name(String::from("overlay"))
        .spawn(move || {
            let mut mixer = Mixer::new(
                &device,
                FORMAT,
                zwcore::output::video::Space::Srgb,
                zwcore::output::video::colour::SDR_WHITE,
            );
            let mut began = std::time::Instant::now();
            let mut shown = 0u64;
            let mut waited = std::time::Duration::ZERO;
            let mut waited_most = std::time::Duration::ZERO;
            // レンダリングできなかった回のバッチはフォントテクスチャの差分を持つので、捨てずに次のバッチへまとめる。
            let mut carried: Option<Box<Drawn>> = None;
            while let Ok(mut drawn) = receive.recv() {
                if let Some(mut older) = carried.take() {
                    drawn.fold_into(&mut older);
                }
                for newer in receive.try_iter() {
                    let mut older = std::mem::replace(&mut drawn, newer);
                    drawn.fold_into(&mut older);
                }
                overlay.configure(&adapter, &device, drawn.size);
                let asked = std::time::Instant::now();
                let frame = overlay.frame();
                let took = asked.elapsed();
                waited += took;
                waited_most = waited_most.max(took);
                if began.elapsed().as_secs() >= 5 {
                    tracing::debug!(
                        "画面のサーフェス {:.1} 秒: 表示 {shown} 枚、取得待ち 合計 {:.0} ms・最大 {:.1} ms",
                        began.elapsed().as_secs_f64(),
                        waited.as_secs_f64() * 1000.0,
                        waited_most.as_secs_f64() * 1000.0
                    );
                    began = std::time::Instant::now();
                    shown = 0;
                    waited = std::time::Duration::ZERO;
                    waited_most = std::time::Duration::ZERO;
                }
                let Some(frame) = frame else {
                    carried = Some(drawn);
                    continue;
                };
                shown += 1;
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let size = (frame.texture.width(), frame.texture.height());
                if !mixer.draw_alone(&device, &queue, &view, size, &mut drawn) {
                    carried = Some(drawn);
                }
                queue.present(frame);
            }
        });
    match spawned {
        Ok(_) => Some(send),
        Err(error) => {
            tracing::warn!("画面のスレッドを起動できない: {error}");
            None
        }
    }
}
