// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デインターレースも逆テレシネも要らないフレームは、元の大きさの RGB を作るパスを飛ばし、表示のパスで
//! デコーダーの 2 プレーンを直に読んで変換と拡大縮小を一回で済ませる。
//! 遅い GPU では、1920x1080 の全面のパスが一回で 3〜4 ms かかる。

use super::*;

/// 表示のパスで直に読むフレーム。
#[derive(Debug, Clone)]
pub(super) struct Direct {
    /// 変換のパスのバインド（直に読めないと分かったら、このフレームの RGB を作る）。
    planes: wgpu::BindGroup,
    luma: wgpu::TextureView,
    chroma: wgpu::TextureView,
}

impl Video {
    /// このフレームを直に読めるか（拡大縮小の方式と、画面へ出す大きさは別に [`Video::direct_fits`]）。
    pub(super) fn can_direct(&self) -> bool {
        #[cfg(feature = "plugins")]
        if !self.hooks.is_empty() {
            return false;
        }
        let filterable = self.format.depth <= 8
            || wgpu::TextureFormat::R16Unorm
                .guaranteed_format_features(self.device.features())
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE);
        self.shape == Shape::Biplanar
            && filterable
            && self.scaling.upscale.code() == 0
            && self.scaling.downscale.code() == 0
    }

    pub(super) fn keep_direct(&mut self, planes: &wgpu::BindGroup, slot: usize) {
        let views = &self.pool.slots[slot].views;
        self.direct = match (views.first(), views.get(1)) {
            (Some(luma), Some(chroma)) => Some(Direct {
                planes: planes.clone(),
                luma: luma.clone(),
                chroma: chroma.clone(),
            }),
            _ => None,
        };
    }

    /// 飛ばした変換のパスを今流す（直に読めない大きさになった、画面を読み取る）。
    pub(super) fn convert_direct(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(direct) = self.direct.take() else {
            return;
        };
        self.ensure_pipeline();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("deinterlace"),
            color_attachments: &[Some(attachment(&self.picture_view))],
            ..Default::default()
        });
        pass.set_pipeline(self.pipeline.as_ref().expect("作ってある"));
        pass.set_bind_group(0, &direct.planes, &[]);
        pass.draw(0..3, 0..1);
    }

    /// 直に読んで描く。描けなければ偽。
    pub(super) fn show_direct(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        onto: Onto,
        rect: [f32; 4],
        shown: [f32; 2],
    ) -> bool {
        let size = [self.format.width, self.format.height];
        self.direct_fits = self.can_direct() && reduce::Reduce::factor(size, shown).is_none();
        if !self.direct_fits || self.shown.is_some() {
            return false;
        }
        let Some(direct) = self.direct.clone() else {
            return false;
        };
        let mapping = self.mapping();
        self.queue.write_buffer(
            &self.fit,
            0,
            &shader::fit_bytes(rect, self.target.is_srgb(), &mapping, self.scaling.antiring),
        );
        let layout = self
            .direct_layout
            .get_or_insert_with(|| direct_layout(&self.device))
            .clone();
        if self.direct_pipeline.is_none() {
            let wide = if self.format.depth > 8 {
                65535.0 / 65472.0
            } else {
                1.0
            };
            self.direct_pipeline = Some(sheet::build_specialised(
                &self.device,
                &layout,
                shader::direct().to_string(),
                self.target,
                "direct",
                &[("WIDE", wide)],
            ));
        }
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("direct"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&direct.luma),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.fit.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&direct.chroma),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        let pipeline = self.direct_pipeline.clone().expect("作った");
        self.show_pass(encoder, view, onto, &pipeline, &group);
        true
    }
}

fn direct_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("direct"),
        entries: &[
            texture(0),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            uniform(2, wgpu::ShaderStages::VERTEX_FRAGMENT),
            texture(3),
            uniform(4, wgpu::ShaderStages::FRAGMENT),
        ],
    })
}
