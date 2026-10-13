// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! フィルターの構成からパイプラインをビルドする。ビルドしたパイプラインはプロセス全体で覚える。

use super::*;

impl Stage {
    pub(super) fn build(
        ctx: &Ctx<'_>,
        name: &str,
        at: At,
        graph: Graph,
        input: ([u32; 2], wgpu::TextureFormat),
        frames: Option<(&wgpu::BindGroupLayout, &str)>,
    ) -> Result<Self, String> {
        graph.check(at)?;
        let device = ctx.device;
        let features = device.features();
        let limit = device.limits().max_texture_dimension_2d;
        let too_big = |size: &[u32]| size.iter().any(|side| *side > limit);
        if too_big(&graph.output) {
            return Err(format!(
                "出力の大きさ {:?} が上限（{limit}）を超えています。",
                graph.output
            ));
        }
        let usage_of = |format: wgpu::TextureFormat| {
            let allowed = format.guaranteed_format_features(features).allowed_usages;
            let mut usage = wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST;
            for extra in [
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                wgpu::TextureUsages::STORAGE_BINDING,
            ] {
                if allowed.contains(extra) {
                    usage |= extra;
                }
            }
            usage
        };

        let mut previous: Vec<String> = Vec::new();
        let mut textures = BTreeMap::new();
        for (key, texture) in &graph.textures {
            let format = format_of(&texture.format).ok_or_else(|| {
                format!("{key}：この GPU は {} に対応していません。", texture.format)
            })?;
            if too_big(&texture.size) {
                return Err(format!(
                    "{key}：{:?} はこの GPU の上限を超えています。",
                    texture.size
                ));
            }
            let tex = match &texture.data {
                Some(hex) => {
                    let bytes = zv::from_hex(hex)
                        .ok_or_else(|| format!("{key}：データが 16 進数ではありません。"))?;
                    let texel = zv::texel_bytes(&texture.format).unwrap_or(4);
                    let count: usize = texture.size.iter().map(|s| *s as usize).product();
                    if bytes.len() != count * texel {
                        return Err(format!(
                            "{key}：データの長さが合いません（{} バイト、{:?} × {texel} なら {} バイト）。",
                            bytes.len(),
                            texture.size,
                            count * texel
                        ));
                    }
                    let tex = Tex::new(
                        device,
                        key,
                        &texture.size,
                        format,
                        texture.linear,
                        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    );
                    ctx.queue.write_texture(
                        tex.texture.as_image_copy(),
                        &bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(tex.size[0] * texel as u32),
                            rows_per_image: Some(tex.size[1]),
                        },
                        wgpu::Extent3d {
                            width: tex.size[0],
                            height: tex.size[1],
                            depth_or_array_layers: tex.size[2],
                        },
                    );
                    tex
                }
                None => Tex::new(
                    device,
                    key,
                    &texture.size,
                    format,
                    texture.linear,
                    usage_of(format),
                ),
            };
            if texture.previous {
                let back = Tex::new(
                    device,
                    &format!("{key}:1"),
                    &texture.size,
                    format,
                    texture.linear,
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                );
                textures.insert(format!("{key}:1"), back);
                previous.push(key.clone());
            }
            textures.insert(key.clone(), tex);
        }
        let outputs = Ring::new(
            (0..=graph.history)
                .map(|_| {
                    Tex::new(
                        device,
                        "output",
                        &graph.output,
                        WORKING,
                        true,
                        usage_of(WORKING),
                    )
                })
                .collect(),
        );
        let inputs = (graph.history > 0).then(|| {
            Ring::new(
                (0..=graph.history)
                    .map(|_| {
                        Tex::new(
                            device,
                            "input",
                            &input.0,
                            input.1,
                            true,
                            wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_DST
                                | wgpu::TextureUsages::COPY_SRC,
                        )
                    })
                    .collect(),
            )
        });

        let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zw_vertex"),
            source: wgpu::ShaderSource::Wgsl(VERTEX.into()),
        });
        let mut passes = Vec::new();
        let mut fresh: Vec<(u64, Cached)> = Vec::new();
        let limits = device.limits();
        let buffers = match make_buffers(ctx, &graph, &limits) {
            Ok(buffers) => buffers,
            Err(why) => {
                let _ = pollster::block_on(scope.pop());
                let _ = pollster::block_on(internal.pop());
                return Err(why);
            }
        };
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zw empty"),
            entries: &[],
        });
        let empty = graph
            .passes
            .iter()
            .any(|pass| !pass.buffers.is_empty())
            .then(|| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("zw empty"),
                    layout: &empty_layout,
                    entries: &[],
                })
            });
        for pass in &graph.passes {
            let (target, format) = if pass.writes == "output" {
                (graph.output, WORKING)
            } else if pass.writes.is_empty() {
                ([0, 0], WORKING)
            } else {
                let tex = &textures[&pass.writes];
                ([tex.size[0], tex.size[1]], tex.format)
            };
            if pass.dispatches()
                && pass
                    .groups
                    .iter()
                    .any(|n| *n > limits.max_compute_workgroups_per_dimension)
            {
                return Err(format!(
                    "{}：ワークグループの数 {:?} が上限（{}）を超えています。",
                    pass.name, pass.groups, limits.max_compute_workgroups_per_dimension
                ));
            }
            if pass.kind == Kind::Host {
                let job = super::host::Job::of(&pass.source).ok_or_else(|| {
                    format!(
                        "{}：{} はこのバージョンでは使えません。",
                        pass.name, pass.source
                    )
                })?;
                passes.push(Built {
                    name: pass.name.clone(),
                    kind: pass.kind,
                    job: Some(job),
                    who: super::host::Job::who(&pass.source).map(str::to_owned),
                    process: pass.launch.clone(),
                    held: pass.held,
                    reads: pass.reads.clone(),
                    writes: pass.writes.clone(),
                    block: pass.block,
                    groups: [0; 3],
                    target,
                    layout: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some(&pass.name),
                        entries: &[],
                    }),
                    render: None,
                    compute: None,
                    zw: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("zw"),
                        size: Zw::SIZE as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    uses_zw: false,
                    fixed: None,
                    buffers: None,
                    slot: None,
                });
                continue;
            }
            let allowed = format.guaranteed_format_features(features).allowed_usages;
            match pass.kind {
                Kind::Fragment if !allowed.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) => {
                    return Err(format!(
                        "{}：この GPU は {format:?} にレンダリングできません。",
                        pass.name
                    ));
                }
                Kind::Compute
                    if !pass.writes.is_empty()
                        && !allowed.contains(wgpu::TextureUsages::STORAGE_BINDING) =>
                {
                    return Err(format!(
                        "{}：この GPU は {format:?} に compute で書き込めません。",
                        pass.name
                    ));
                }
                Kind::Compute if device.limits().max_compute_workgroups_per_dimension == 0 => {
                    return Err(format!(
                        "{}：この GPU は compute に対応していません。",
                        pass.name
                    ));
                }
                _ => {}
            }
            let visibility = wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE;
            let mut entries = vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }];
            for (i, read) in pass.reads.iter().enumerate() {
                let (dimension, linear) = match Source::of(read) {
                    Source::Named(name) if textures.contains_key(name) => {
                        let tex = &textures[name];
                        (tex.dimension, tex.linear)
                    }
                    _ => (wgpu::TextureViewDimension::D2, true),
                };
                let binding = 1 + 2 * i as u32;
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: linear },
                        view_dimension: dimension,
                        multisampled: false,
                    },
                    count: None,
                });
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: binding + 1,
                    visibility,
                    ty: wgpu::BindingType::Sampler(if linear {
                        wgpu::SamplerBindingType::Filtering
                    } else {
                        wgpu::SamplerBindingType::NonFiltering
                    }),
                    count: None,
                });
            }
            if pass.kind == Kind::Compute && !pass.writes.is_empty() {
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 1 + 2 * pass.reads.len() as u32,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                });
            }
            let source = match frames {
                Some((_, prelude)) => format!("{prelude}\n{}", pass.source),
                None => pass.source.clone(),
            };
            let bound = bind_entries(pass, &graph, &limits)?;
            let uses_zw = source.contains("zw");
            let key = pipeline_key(
                &source,
                &pass.entry,
                pass.kind,
                format,
                &entries,
                &bound,
                pass.zero_workgroup,
            );
            let zw = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("zw"),
                size: Zw::SIZE as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let found = fresh
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, cached)| cached.clone())
                .or_else(|| cached_pipeline(device, key));
            let cached = match found {
                Some(cached) => cached,
                None => {
                    let cached = make_pipeline(
                        device,
                        pass,
                        source,
                        format,
                        &entries,
                        &bound,
                        &vertex,
                        frames.map(|(frames, _)| frames),
                        &empty_layout,
                    );
                    fresh.push((key, cached.clone()));
                    cached
                }
            };
            let groups_layout = cached.buffers.clone();
            let mut built = Built {
                name: pass.name.clone(),
                kind: pass.kind,
                job: None,
                who: None,
                process: None,
                held: 0,
                reads: pass.reads.clone(),
                writes: pass.writes.clone(),
                block: pass.block,
                groups: if pass.dispatches() {
                    pass.groups
                } else {
                    [0; 3]
                },
                target,
                layout: cached.layout,
                render: cached.render,
                compute: cached.compute,
                zw,
                uses_zw,
                fixed: None,
                buffers: None,
                slot: None,
            };
            if let Some(layout) = groups_layout {
                built.buffers = Some(bind_buffers(device, pass, &layout, &buffers));
            }
            passes.push(built);
        }
        let validation = pollster::block_on(scope.pop());
        let inside = pollster::block_on(internal.pop());
        if let Some(error) = validation.or(inside) {
            return Err(first_lines(&error.to_string()));
        }
        remember_pipelines(device, fresh);
        // 続けて記録するパスは最後の一つだけがタイムスタンプのスロットを持つ（`ZW_FILTER_SPLIT` ならパスごとに測る）。
        let split = std::env::var_os("ZW_FILTER_SPLIT").is_some();
        let mut slots = 0;
        for i in 0..passes.len() {
            let folded = !split
                && passes[i].groups != [0; 3]
                && passes.get(i + 1).is_some_and(|next| next.groups != [0; 3]);
            if !folded {
                passes[i].slot = Some(slots);
                slots += 1;
            }
        }
        let timing = timing::Timing::new(device, ctx.queue, slots);
        Ok(Self {
            name: name.to_owned(),
            graph,
            passes,
            textures,
            empty,
            previous,
            inputs,
            outputs,
            input_size: input.0,
            judging: true,
            measured: timing.is_none().then(Vec::new),
            ms: None,
            marker: None,
            timing,
            host_failed: false,
            host_failure: None,
            over: false,
        })
    }
}

#[derive(Clone)]
struct Cached {
    layout: wgpu::BindGroupLayout,
    render: Option<wgpu::RenderPipeline>,
    compute: Option<wgpu::ComputePipeline>,
    buffers: Option<wgpu::BindGroupLayout>,
}

fn make_buffers(
    ctx: &Ctx<'_>,
    graph: &Graph,
    limits: &wgpu::Limits,
) -> Result<BTreeMap<String, wgpu::Buffer>, String> {
    let mut made = BTreeMap::new();
    for (name, buffer) in &graph.buffers {
        if buffer.size > limits.max_buffer_size {
            return Err(format!(
                "{name}：{} バイトはこの GPU の上限（{}）を超えています。",
                buffer.size, limits.max_buffer_size
            ));
        }
        let made_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(name),
            size: buffer.size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bytes: Option<Vec<u8>> = match (&buffer.data, &buffer.blob) {
            (Some(hex), _) => Some(
                zv::from_hex(hex)
                    .ok_or_else(|| format!("{name}：データが 16 進数ではありません。"))?,
            ),
            (None, Some(at)) => {
                let blob = graph
                    .blobs
                    .get(&at.name)
                    .ok_or_else(|| format!("{name}：blob {} がありません。", at.name))?;
                let from = (at.offset as usize).min(blob.0.len());
                let to = from.saturating_add(buffer.size as usize).min(blob.0.len());
                Some(blob.0[from..to].to_vec())
            }
            (None, None) => None,
        };
        if let Some(mut bytes) = bytes {
            bytes.truncate(buffer.size as usize);
            bytes.resize(bytes.len().div_ceil(4) * 4, 0);
            if !bytes.is_empty() {
                ctx.queue.write_buffer(&made_buffer, 0, &bytes);
            }
        }
        made.insert(name.clone(), made_buffer);
    }
    Ok(made)
}

fn bind_entries(
    pass: &zv::Pass,
    graph: &Graph,
    limits: &wgpu::Limits,
) -> Result<Vec<wgpu::BindGroupLayoutEntry>, String> {
    let storage = pass
        .buffers
        .iter()
        .filter(|bind| bind.access != zv::Access::Uniform)
        .count() as u32;
    if storage > limits.max_storage_buffers_per_shader_stage {
        return Err(format!(
            "{}：storage バッファが {storage} 個で、この GPU の上限（{}）を超えています。",
            pass.name, limits.max_storage_buffers_per_shader_stage
        ));
    }
    let mut entries = Vec::new();
    for bind in &pass.buffers {
        let size = graph.buffers.get(&bind.name).map_or(0, |b| b.size);
        let length = if bind.size == 0 {
            size.saturating_sub(bind.offset)
        } else {
            bind.size
        };
        let (align, most) = match bind.access {
            zv::Access::Uniform => (
                limits.min_uniform_buffer_offset_alignment,
                limits.max_uniform_buffer_binding_size,
            ),
            _ => (
                limits.min_storage_buffer_offset_alignment,
                limits.max_storage_buffer_binding_size,
            ),
        };
        if !bind.offset.is_multiple_of(u64::from(align.max(1))) {
            return Err(format!(
                "{}：{} の位置 {} が {align} の倍数ではありません。",
                pass.name, bind.name, bind.offset
            ));
        }
        if length > most {
            return Err(format!(
                "{}：{} の {length} バイトはこの GPU の上限（{most}）を超えています。",
                pass.name, bind.name
            ));
        }
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: bind.binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: match bind.access {
                    zv::Access::Uniform => wgpu::BufferBindingType::Uniform,
                    zv::Access::Read => wgpu::BufferBindingType::Storage { read_only: true },
                    zv::Access::ReadWrite => wgpu::BufferBindingType::Storage { read_only: false },
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
    }
    Ok(entries)
}

fn bind_buffers(
    device: &wgpu::Device,
    pass: &zv::Pass,
    layout: &wgpu::BindGroupLayout,
    buffers: &BTreeMap<String, wgpu::Buffer>,
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = pass
        .buffers
        .iter()
        .map(|bind| wgpu::BindGroupEntry {
            binding: bind.binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffers[&bind.name],
                offset: bind.offset,
                size: std::num::NonZeroU64::new(bind.size),
            }),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&pass.name),
        layout,
        entries: &entries,
    })
}

#[allow(clippy::too_many_arguments)]
fn make_pipeline(
    device: &wgpu::Device,
    pass: &zv::Pass,
    source: String,
    format: wgpu::TextureFormat,
    entries: &[wgpu::BindGroupLayoutEntry],
    bound: &[wgpu::BindGroupLayoutEntry],
    vertex: &wgpu::ShaderModule,
    frames: Option<&wgpu::BindGroupLayout>,
    empty: &wgpu::BindGroupLayout,
) -> Cached {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(&pass.name),
        entries,
    });
    let buffers = (!bound.is_empty()).then(|| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&pass.name),
            entries: bound,
        })
    });
    let mut groups: Vec<Option<&wgpu::BindGroupLayout>> = vec![Some(&layout)];
    if frames.is_some() || buffers.is_some() {
        groups.push(Some(frames.unwrap_or(empty)));
    }
    if let Some(buffers) = &buffers {
        groups.push(Some(buffers));
    }
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&pass.name),
        bind_group_layouts: &groups,
        immediate_size: 0,
    });
    // DXC はラベルをファイル名として読むので、`|` や日本語を含めない。
    let label = ascii_label(&pass.name);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let (render, compute) = match pass.kind {
        Kind::Fragment => (
            Some(
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&label),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: vertex,
                        entry_point: Some("zw_vertex"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(&pass.entry),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                }),
            ),
            None,
        ),
        Kind::Host => (None, None),
        Kind::Compute => (
            None,
            Some(
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(&label),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(&pass.entry),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[],
                        zero_initialize_workgroup_memory: pass.zero_workgroup,
                    },
                    cache: None,
                }),
            ),
        ),
    };
    Cached {
        layout,
        render,
        compute,
        buffers,
    }
}

/// ビルドしたパイプラインをプロセス全体で覚える（D3D12 の FXC は十数秒かかるため）。
static PIPELINES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<(wgpu::Device, u64), Cached>>,
> = std::sync::LazyLock::new(Default::default);

const MOST_PIPELINES: usize = 256;

fn pipeline_key(
    source: &str,
    entry: &str,
    kind: Kind,
    format: wgpu::TextureFormat,
    entries: &[wgpu::BindGroupLayoutEntry],
    bound: &[wgpu::BindGroupLayoutEntry],
    zero_workgroup: bool,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    entry.hash(&mut hasher);
    (kind == Kind::Compute).hash(&mut hasher);
    zero_workgroup.hash(&mut hasher);
    format!("{format:?}{entries:?}{bound:?}").hash(&mut hasher);
    hasher.finish()
}

fn cached_pipeline(device: &wgpu::Device, key: u64) -> Option<Cached> {
    let cache = PIPELINES.lock().ok()?;
    cache.get(&(device.clone(), key)).cloned()
}

fn remember_pipelines(device: &wgpu::Device, fresh: Vec<(u64, Cached)>) {
    let Ok(mut cache) = PIPELINES.lock() else {
        return;
    };
    cache.retain(|(owner, _), _| owner == device);
    if cache.len() + fresh.len() > MOST_PIPELINES {
        cache.clear();
    }
    for (key, cached) in fresh {
        cache.insert((device.clone(), key), cached);
    }
}

fn ascii_label(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn first_lines(text: &str) -> String {
    text.lines().take(12).collect::<Vec<_>>().join("\n")
}

pub(super) fn frames_layout(
    device: &wgpu::Device,
    depth: u8,
    shape: Shape,
    [lo, hi]: [i32; 2],
) -> wgpu::BindGroupLayout {
    let uint = depth > 8 && shape == Shape::Planar;
    let count = (hi - lo + 1) as u32 * shape.textures();
    let visibility = wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..count)
        .map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: if uint {
                    wgpu::TextureSampleType::Uint
                } else {
                    wgpu::TextureSampleType::Float { filterable: false }
                },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        })
        .collect();
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: count,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("zw frames"),
        entries: &entries,
    })
}

pub(super) fn copy_pipeline(
    device: &wgpu::Device,
    picture: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("zw copy"),
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
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("zw copy"),
        source: wgpu::ShaderSource::Wgsl(format!("{VERTEX}{COPY}").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("zw copy"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("zw copy"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("zw_vertex"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("zw_copy"),
            targets: &[Some(wgpu::ColorTargetState {
                format: picture,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    (pipeline, layout)
}
