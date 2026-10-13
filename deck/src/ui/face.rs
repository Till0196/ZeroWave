// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 一つのウィンドウに付く画面（egui と winit・wgpu をつなぐ）。
//! eframe のマルチビューポートは使わない（一本のスレッドでレンダリングすると垂直同期の待ちが映像に響く）。

use egui_wgpu::ScreenDescriptor;
use winit::window::Window;

pub struct Face {
    context: egui::Context,
    fonts_seen: u64,
    /// ウィンドウのイベントを egui の入力に直す側。別ウィンドウだけが持つ。
    state: Option<egui_winit::State>,
    renderer: Renderer,
    next: Option<std::time::Instant>,
}

enum Renderer {
    Wgpu(Box<egui_wgpu::Renderer>),
    /// レンダラーはコンテキストを有効にしてからでないと作れないので、最初にレンダリングするときに作る。
    #[cfg(all(unix, not(target_os = "macos")))]
    Glow(Option<egui_glow::Painter>),
}

impl Renderer {
    /// GL のときは `None`（Windows と macOS では wgpu だけなので常に `Some`）。
    fn wgpu(&mut self) -> Option<&mut egui_wgpu::Renderer> {
        match self {
            Self::Wgpu(renderer) => Some(&mut **renderer),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::Glow(_) => None,
        }
    }
}

impl std::fmt::Debug for Face {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Face").finish_non_exhaustive()
    }
}

impl Face {
    pub fn new(window: &Window, device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let renderer =
            egui_wgpu::Renderer::new(device, target, egui_wgpu::RendererOptions::default());
        Self::with(window, Renderer::Wgpu(Box::new(renderer)), None)
    }

    /// GLES 2.0 のウィンドウの画面。VC4 の GL は 2048 までのテクスチャしか持てない。
    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn on_gles(window: &Window, paper: Option<super::theme::Mode>) -> Self {
        let face = Self::with(window, Renderer::Glow(None), Some(2048));
        if let Some(mode) = paper {
            super::paper(&face.context, mode);
        }
        face
    }

    fn with(window: &Window, renderer: Renderer, max_texture_side: Option<usize>) -> Self {
        let context = egui::Context::default();
        crate::ui::text::font::install_or_say(&context);
        super::style(&context, super::theme::Mode::Dark);
        let state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            max_texture_side,
        );
        Self {
            fonts_seen: crate::ui::text::font::generation(),
            context,
            state: Some(state),
            renderer,
            next: Some(std::time::Instant::now()),
        }
    }

    pub fn on_paper(
        window: &Window,
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        mode: super::theme::Mode,
    ) -> Self {
        let face = Self::new(window, device, target);
        super::paper(&face.context, mode);
        face
    }

    pub fn repaper(&self, mode: super::theme::Mode) {
        super::paper(&self.context, mode);
    }

    pub fn copy_text(&self, text: String) {
        self.context.copy_text(text);
    }

    pub fn on_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state
            .as_mut()
            .is_some_and(|state| state.on_window_event(window, event).consumed)
    }

    pub fn take_input(&mut self, window: &Window) -> egui::RawInput {
        self.state
            .as_mut()
            .map(|state| state.take_egui_input(window))
            .unwrap_or_default()
    }

    pub fn hand_output(&mut self, window: &Window, output: egui::PlatformOutput) {
        if let Some(state) = self.state.as_mut() {
            state.handle_platform_output(window, output);
        }
    }

    /// 呼ぶ側が取ったサーフェス（映像をレンダリングしたあと）へ重ねてレンダリングする。GLES 2.0 のウィンドウなら何もしない。
    #[cfg_attr(target_os = "macos", allow(irrefutable_let_patterns))]
    pub fn draw(
        &mut self,
        input: egui::RawInput,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size: (u32, u32),
        build: impl FnMut(&mut egui::Ui),
    ) -> egui::PlatformOutput {
        let Some(renderer) = self.renderer.wgpu() else {
            return egui::PlatformOutput::default();
        };
        let output = run(
            &self.context,
            &mut self.fonts_seen,
            &mut self.next,
            input,
            build,
        );
        let jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(device, queue, *id, delta);
            }
        }
        let descriptor = ScreenDescriptor {
            size_in_pixels: [size.0, size.1],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("画面"),
        });
        renderer.update_buffers(device, queue, &mut encoder, &jobs, &descriptor);
        {
            // `Load`：サーフェスには映像が既にレンダリングされている。
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("画面"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &descriptor);
        }
        queue.submit([encoder.finish()]);
        for id in &output.textures_delta.free {
            renderer.free_texture(id);
        }
        output.platform_output
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn draw_gles(
        &mut self,
        input: egui::RawInput,
        window: &zwcore::output::present::Window,
        build: impl FnMut(&mut egui::Ui),
    ) -> egui::PlatformOutput {
        let output = run(
            &self.context,
            &mut self.fonts_seen,
            &mut self.next,
            input,
            build,
        );
        let jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        let mut delta = output.textures_delta;
        let Renderer::Glow(painter) = &mut self.renderer else {
            delta.clear();
            return output.platform_output;
        };
        if let Err(error) = window.make_current() {
            tracing::warn!("別ウィンドウの GL コンテキストに切り替えられない: {error:?}");
            delta.clear();
            return output.platform_output;
        }
        if painter.is_none() {
            // SAFETY: コンテキストは直上で有効にした。関数ポインタはそのコンテキストの物。
            let gl = unsafe {
                glow::Context::from_loader_function_cstr(|name| window.proc_address(name))
            };
            match egui_glow::Painter::new(std::sync::Arc::new(gl), "", None, false) {
                Ok(made) => *painter = Some(made),
                Err(error) => tracing::warn!("別ウィンドウのレンダラーを作成できない: {error}"),
            }
        }
        let Some(painter) = painter.as_mut() else {
            delta.clear();
            return output.platform_output;
        };
        let size = window.follow();
        // 前のフレームの上にはレンダリングしない（egui の背景が塗らない隅が残る）。
        painter.clear([size.0, size.1], [0.0, 0.0, 0.0, 1.0]);
        painter.paint_and_update_textures(
            [size.0, size.1],
            output.pixels_per_point,
            &jobs,
            &mut delta,
        );
        delta.clear();
        if let Err(error) = window.swap() {
            tracing::warn!("別ウィンドウのバッファーを入れ替えられない: {error:?}");
        }
        output.platform_output
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn release_gl(&mut self) {
        if let Renderer::Glow(painter) = &mut self.renderer
            && let Some(mut painter) = painter.take()
        {
            painter.destroy();
        }
    }

    /// `has_requested_repaint` だと「250ms 後に」を「いますぐ」と読み、スレッドがスリープできなくなる。
    pub fn due_by(&self, when: std::time::Instant) -> bool {
        self.next.is_some_and(|next| next <= when)
    }

    pub fn due_in(&self, from: std::time::Instant) -> Option<std::time::Duration> {
        self.next.map(|next| next.saturating_duration_since(from))
    }
}

fn run(
    context: &egui::Context,
    fonts_seen: &mut u64,
    next: &mut Option<std::time::Instant>,
    input: egui::RawInput,
    build: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    crate::ui::text::font::refresh(context, fonts_seen);
    let output = context.run_ui(input, build);
    *next = output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .and_then(|viewport| std::time::Instant::now().checked_add(viewport.repaint_delay));
    output
}
