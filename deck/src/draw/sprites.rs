// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインのレイヤーの絵の並び（[`zwcore::layers::Picture::Sprites`]）を、egui の描画の中で GPU で重ねる。
//! 画面（egui）が置くのはレイヤーの場所だけで、位置と絵は提示する直前に最新のレイヤーから読む。
//! 絵が動くだけなら画面を作り直さず、テクスチャのアップロードも絵が変わったときだけになる。

use std::collections::HashMap;
use std::sync::Arc;

use zwcore::layers::Latest;
use zwsdk::host::SpriteImage;

/// レンダーパスの形式。[`super::overlay::Mixer`] が egui のレンダラーに入れる。
pub struct Target(pub wgpu::TextureFormat);

/// これから提示する画面の垂直同期の時刻。[`super::overlay::Mixer`] が描く前に入れる。
pub struct PresentAt(pub std::time::Instant);

/// 一枚の頂点：位置（ビューポートの正規化座標）、テクスチャの座標、不透明度。
#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    at: [f32; 2],
    uv: [f32; 2],
    alpha: f32,
}

pub struct Callback {
    pub layer: u64,
    pub latest: Latest,
}

struct Held {
    group: wgpu::BindGroup,
    /// 同じアドレスが別の絵に使い回されないように持つ。
    _image: Arc<SpriteImage>,
    used: u64,
}

struct Draw {
    texture: usize,
    first: u32,
    clip: [f32; 4],
}

struct Prepared {
    vertices: wgpu::Buffer,
    draws: Vec<Draw>,
    area: (f32, f32),
}

struct Sprites {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<usize, Held>,
    prepared: HashMap<u64, Prepared>,
    turn: u64,
}

impl Sprites {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer sprites"),
            source: wgpu::ShaderSource::Wgsl(zwcore::output::video::shader::sprites().into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("レイヤーの絵"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("レイヤーの絵"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer sprites"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("レイヤーの絵"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
            textures: HashMap::new(),
            prepared: HashMap::new(),
            turn: 0,
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &Arc<SpriteImage>,
    ) -> usize {
        let key = Arc::as_ptr(image) as usize;
        let turn = self.turn;
        if let Some(held) = self.textures.get_mut(&key) {
            held.used = turn;
            return key;
        }
        let size = wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("レイヤーの絵"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: None,
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("レイヤーの絵"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.textures.insert(
            key,
            Held {
                group,
                _image: Arc::clone(image),
                used: turn,
            },
        );
        key
    }
}

/// 使われなくなった絵をこれだけの準備の後に手放す（同じ準備の中のほかのレイヤーが使っているかもしれない）。
const KEEP_TURNS: u64 = 8;

/// 置く矩形と切り抜く矩形を、提示の時刻に合わせて一つ前の並びとの間で補間する。プラグインが作った時刻
/// （`time`）の絵を、その一枚分の間隔だけ遅れて見せる（同じ fps なら今のまま、低い fps でも提示ごとに進む）。
/// 今の一枚より先へは延ばさない（次の一枚が遅れて届いたときに戻って見える）。
/// 同じ絵は、同じ画像の何枚目かで突き合わせる。一つ前に無い絵はそのまま置く。
fn moved(
    layer: &zwcore::layers::Layer,
    present_at: Option<std::time::Instant>,
) -> Vec<[[f32; 4]; 2]> {
    let placed = layer.sprites().unwrap_or_default();
    let now = |sprite: &zwsdk::host::Placed| [sprite.at, sprite.clip];
    let (Some(present_at), Some((time, Some(earlier)))) = (present_at, layer.motion()) else {
        return placed.iter().map(now).collect();
    };
    let span = time.saturating_duration_since(earlier.time).as_secs_f32();
    if span <= 0.0 {
        return placed.iter().map(now).collect();
    }
    let u = (present_at.saturating_duration_since(time).as_secs_f32() / span).min(1.0);
    let mut nth: HashMap<usize, usize> = HashMap::new();
    let mut before: HashMap<(usize, usize), &zwsdk::host::Placed> = HashMap::new();
    for sprite in &earlier.sprites {
        let key = Arc::as_ptr(&sprite.image) as usize;
        let n = nth.entry(key).or_default();
        before.insert((key, *n), sprite);
        *n += 1;
    }
    nth.clear();
    placed
        .iter()
        .map(|sprite| {
            let key = Arc::as_ptr(&sprite.image) as usize;
            let n = nth.entry(key).or_default();
            let was = before.get(&(key, *n));
            *n += 1;
            match was {
                Some(was) => {
                    let lerp = |a: [f32; 4], b: [f32; 4]| {
                        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * u)
                    };
                    [lerp(was.at, sprite.at), lerp(was.clip, sprite.clip)]
                }
                None => now(sprite),
            }
        })
        .collect()
}

impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(format) = resources.get::<Target>().map(|target| target.0) else {
            return Vec::new();
        };
        let present_at = resources.get::<PresentAt>().map(|at| at.0);
        let sprites = resources
            .entry::<Sprites>()
            .or_insert_with(|| Sprites::new(device, format));
        sprites.turn += 1;
        let turn = sprites.turn;
        sprites
            .textures
            .retain(|_, held| turn.saturating_sub(held.used) < KEEP_TURNS);
        sprites.prepared.remove(&self.layer);
        let layers = Arc::clone(&self.latest.lock().unwrap_or_else(|e| e.into_inner()));
        let Some(layer) = layers.iter().find(|layer| layer.id == self.layer) else {
            return Vec::new();
        };
        let Some(placed) = layer.sprites() else {
            return Vec::new();
        };
        let area = (layer.frame.width as f32, layer.frame.height as f32);
        if area.0 <= 0.0 || area.1 <= 0.0 || placed.is_empty() {
            return Vec::new();
        }
        let mut vertices = Vec::with_capacity(placed.len() * 6);
        let mut draws = Vec::with_capacity(placed.len());
        for (sprite, [at, clip]) in placed.iter().zip(moved(layer, present_at)) {
            let texture = sprites.upload(device, queue, &sprite.image);
            let [x, y, w, h] = at;
            let ndc = |px: f32, py: f32| [px / area.0 * 2.0 - 1.0, 1.0 - py / area.1 * 2.0];
            let corner = |cx: f32, cy: f32| Vertex {
                at: ndc(x + cx * w, y + cy * h),
                uv: [cx, cy],
                alpha: sprite.alpha,
            };
            draws.push(Draw {
                texture,
                first: vertices.len() as u32,
                clip,
            });
            vertices.extend([
                corner(0.0, 0.0),
                corner(1.0, 0.0),
                corner(0.0, 1.0),
                corner(0.0, 1.0),
                corner(1.0, 0.0),
                corner(1.0, 1.0),
            ]);
        }
        let bytes: Vec<u8> = vertices
            .iter()
            .flat_map(|vertex| {
                vertex
                    .at
                    .iter()
                    .chain(&vertex.uv)
                    .chain(std::iter::once(&vertex.alpha))
                    .flat_map(|value| value.to_ne_bytes())
                    .collect::<Vec<u8>>()
            })
            .collect();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("レイヤーの絵"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &bytes);
        sprites.prepared.insert(
            self.layer,
            Prepared {
                vertices: buffer,
                draws,
                area,
            },
        );
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(sprites) = resources.get::<Sprites>() else {
            return;
        };
        let Some(prepared) = sprites.prepared.get(&self.layer) else {
            return;
        };
        let viewport = info.viewport_in_pixels();
        let outer = info.clip_rect_in_pixels();
        let (screen_w, screen_h) = (info.screen_size_px[0] as i32, info.screen_size_px[1] as i32);
        let sx = viewport.width_px as f32 / prepared.area.0;
        let sy = viewport.height_px as f32 / prepared.area.1;
        pass.set_pipeline(&sprites.pipeline);
        pass.set_vertex_buffer(0, prepared.vertices.slice(..));
        for draw in &prepared.draws {
            let Some(held) = sprites.textures.get(&draw.texture) else {
                continue;
            };
            let [cx, cy, cw, ch] = draw.clip;
            let left = (viewport.left_px as f32 + cx * sx).floor() as i32;
            let top = (viewport.top_px as f32 + cy * sy).floor() as i32;
            let right = (viewport.left_px as f32 + (cx + cw) * sx).ceil() as i32;
            let bottom = (viewport.top_px as f32 + (cy + ch) * sy).ceil() as i32;
            let left = left.max(outer.left_px).max(0);
            let top = top.max(outer.top_px).max(0);
            let right = right.min(outer.left_px + outer.width_px).min(screen_w);
            let bottom = bottom.min(outer.top_px + outer.height_px).min(screen_h);
            if right <= left || bottom <= top {
                continue;
            }
            pass.set_scissor_rect(
                left as u32,
                top as u32,
                (right - left) as u32,
                (bottom - top) as u32,
            );
            pass.set_bind_group(0, &held.group, &[]);
            pass.draw(draw.first..draw.first + 6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn placed(image: &Arc<SpriteImage>, x: f32) -> zwsdk::host::Placed {
        zwsdk::host::Placed {
            image: Arc::clone(image),
            at: [x, 0.0, 10.0, 10.0],
            clip: [x, 0.0, 10.0, 10.0],
            alpha: 1.0,
        }
    }

    #[test]
    fn sprites_move_by_the_time_of_the_present() {
        let image = Arc::new(SpriteImage {
            width: 10,
            height: 10,
            rgba: vec![0; 400],
        });
        let other = Arc::new(SpriteImage {
            width: 10,
            height: 10,
            rgba: vec![0; 400],
        });
        let then = Instant::now();
        let time = then + Duration::from_millis(40);
        let layer = zwcore::layers::Layer {
            name: String::new(),
            id: 1,
            place: zwsdk::layer::Place::At(zwsdk::layer::Slot::Captions),
            frame: zwsdk::layer::Frame::default(),
            picture: zwcore::layers::Picture::Sprites {
                sprites: vec![placed(&image, 100.0), placed(&other, 7.0)],
                time,
                earlier: Some(zwcore::layers::Earlier {
                    time: then,
                    sprites: vec![placed(&image, 200.0)],
                }),
                image: std::sync::OnceLock::new(),
            },
        };
        let x = |after: u64| moved(&layer, Some(time + Duration::from_millis(after)))[0][0][0];
        assert!((x(0) - 200.0).abs() < 0.01);
        assert!((x(20) - 150.0).abs() < 0.01);
        assert!((x(40) - 100.0).abs() < 0.01);
        assert!(
            (x(200) - 100.0).abs() < 0.01,
            "今の一枚より先へは延ばさない"
        );
        assert_eq!(moved(&layer, Some(time))[1][0][0], 7.0);
        assert_eq!(moved(&layer, None)[0][0][0], 100.0);
    }
}
