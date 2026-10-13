// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! オーバーレイ。ビルドするのはメインスレッド、レンダリングするのは再生スレッド（渡すのは図形）。
//! フォントテクスチャの差分は届いた時に適用し、バッチを差し替えた時に解放する（古いバッチをレンダリングし続けることがある）。

use zwcore::output::video::Space;
use zwcore::output::video::gpu::Sheet;

pub struct Drawn {
    jobs: Vec<egui::ClippedPrimitive>,
    textures: egui::TexturesDelta,
    pixels_per_point: f32,
    pub size: (u32, u32),
    painted: bool,
}

impl Drawn {
    pub fn new(
        jobs: Vec<egui::ClippedPrimitive>,
        textures: egui::TexturesDelta,
        pixels_per_point: f32,
        size: (u32, u32),
    ) -> Self {
        Self {
            jobs,
            textures,
            pixels_per_point,
            size,
            painted: false,
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn into_parts(
        mut self,
    ) -> (
        Vec<egui::ClippedPrimitive>,
        egui::TexturesDelta,
        f32,
        (u32, u32),
    ) {
        (
            std::mem::take(&mut self.jobs),
            std::mem::take(&mut self.textures),
            self.pixels_per_point,
            self.size,
        )
    }

    /// 古いバッチをこのバッチへまとめる。古いほうの差分を捨てると `egui_wgpu` が落ちる。
    pub fn fold_into(&mut self, older: &mut Self) {
        let mut set = std::mem::take(&mut older.textures.set);
        for (id, mut deltas) in std::mem::take(&mut self.textures.set) {
            set.entry(id).or_default().append(&mut deltas);
        }
        self.textures.set = set;
        self.textures
            .free
            .extend(std::mem::take(&mut older.textures.free));
        let free = &self.textures.free;
        self.textures.set.retain(|id, _| !free.contains(id));
    }
}

impl std::fmt::Debug for Drawn {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Drawn")
            .field("size", &self.size)
            .field("jobs", &self.jobs.len())
            .finish_non_exhaustive()
    }
}

/// 再生スレッドの側。画面はいつもシート（[`Sheet`]）へレンダリングしてからサーフェスへコピーする
/// （SDR と HDR を切り替えてもレンダラーを作り直さないため）。
pub struct Mixer {
    renderer: egui_wgpu::Renderer,
    sheet: Sheet,
    area: Option<[u32; 4]>,
    blank: bool,
}

impl std::fmt::Debug for Mixer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Mixer").finish_non_exhaustive()
    }
}

impl Mixer {
    pub fn new(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        space: Space,
        white: f32,
    ) -> Self {
        Self {
            renderer: egui_wgpu::Renderer::new(
                device,
                Sheet::FORMAT,
                egui_wgpu::RendererOptions::default(),
            ),
            sheet: Sheet::new(device, target, space, white, (1, 1)),
            area: None,
            blank: true,
        }
    }

    /// サーフェスの形式か色空間が変わった。`white` は SDR の白の高さ（cd/m²）。
    pub fn retarget(
        &mut self,
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        space: Space,
        white: f32,
    ) {
        self.sheet = Sheet::new(device, target, space, white, self.sheet.size());
        self.blank = true;
    }

    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        onto: &wgpu::Texture,
        size: (u32, u32),
        drawn: &mut Drawn,
    ) {
        super::unless_lost(|| self.draw_now(device, queue, onto, size, drawn));
    }

    fn draw_now(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        onto: &wgpu::Texture,
        size: (u32, u32),
        drawn: &mut Drawn,
    ) {
        // ビルドしたときと大きさが違う。差分は保持したまま次のバッチへまとめる。
        if drawn.size != size {
            return;
        }
        let repaint = !drawn.painted
            || self.blank
            || !drawn.textures.set.is_empty()
            || !drawn.textures.free.is_empty()
            || self.sheet.size() != size;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("画面"),
        });
        if repaint {
            let descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [size.0, size.1],
                pixels_per_point: drawn.pixels_per_point,
            };
            // フォントテクスチャはレンダリング前に適用し、レンダリング後に解放する（egui の決まり）。
            let textures = std::mem::take(&mut drawn.textures);
            for (id, deltas) in &textures.set {
                for delta in deltas {
                    self.renderer.update_texture(device, queue, *id, delta);
                }
            }
            self.renderer
                .update_buffers(device, queue, &mut encoder, &drawn.jobs, &descriptor);
            self.sheet.resize(device, size);
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("画面"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: self.sheet.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                self.renderer
                    .render(&mut pass.forget_lifetime(), &drawn.jobs, &descriptor);
            }
            drawn.painted = true;
            self.blank = false;
            self.area = covered(&drawn.jobs, drawn.pixels_per_point, size);
            for id in &textures.free {
                self.renderer.free_texture(id);
            }
        }
        if let Some(area) = self.area {
            self.sheet
                .lay(device, queue, &mut encoder, onto, Some(area));
        }
        queue.submit([encoder.finish()]);
    }

    /// レンダリングしなかったら偽（差分は次のバッチへまとめる）。
    #[cfg(target_os = "windows")]
    pub fn draw_alone(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        onto: &wgpu::TextureView,
        size: (u32, u32),
        drawn: &mut Drawn,
    ) -> bool {
        super::unless_lost(|| self.draw_alone_now(device, queue, onto, size, drawn))
            .unwrap_or(false)
    }

    #[cfg(target_os = "windows")]
    fn draw_alone_now(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        onto: &wgpu::TextureView,
        size: (u32, u32),
        drawn: &mut Drawn,
    ) -> bool {
        if drawn.size != size {
            return false;
        }
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.0, size.1],
            pixels_per_point: drawn.pixels_per_point,
        };
        let textures = std::mem::take(&mut drawn.textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("画面（別）"),
        });
        self.renderer
            .update_buffers(device, queue, &mut encoder, &drawn.jobs, &descriptor);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("画面（別）"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: onto,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &drawn.jobs, &descriptor);
        }
        queue.submit([encoder.finish()]);
        for id in &textures.free {
            self.renderer.free_texture(id);
        }
        true
    }
}

fn covered(
    jobs: &[egui::ClippedPrimitive],
    pixels_per_point: f32,
    size: (u32, u32),
) -> Option<[u32; 4]> {
    let mut all = egui::Rect::NOTHING;
    for job in jobs {
        let shape = match &job.primitive {
            egui::epaint::Primitive::Mesh(mesh) => {
                if mesh.indices.is_empty() {
                    continue;
                }
                mesh.vertices.iter().fold(egui::Rect::NOTHING, |r, v| {
                    r.union(egui::Rect::from_min_max(v.pos, v.pos))
                })
            }
            egui::epaint::Primitive::Callback(_) => job.clip_rect,
        };
        let part = shape.intersect(job.clip_rect);
        if part.is_positive() {
            all = all.union(part);
        }
    }
    if !all.is_positive() {
        return None;
    }
    let x0 = ((all.min.x * pixels_per_point).floor() as i64 - 1).clamp(0, i64::from(size.0)) as u32;
    let y0 = ((all.min.y * pixels_per_point).floor() as i64 - 1).clamp(0, i64::from(size.1)) as u32;
    let x1 = ((all.max.x * pixels_per_point).ceil() as i64 + 1).clamp(0, i64::from(size.0)) as u32;
    let y1 = ((all.max.y * pixels_per_point).ceil() as i64 + 1).clamp(0, i64::from(size.1)) as u32;
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}
