// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! デコードしたフレームを GPU のテクスチャへ入れる方法（[`Upload`]）と、そのテクスチャのプール（[`Pool`]）。

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Upload {
    /// `write_texture` で、ドライバーのステージングを経て GPU でコピーする。
    #[default]
    Staged,
    /// マップしたままのバッファーへ書き、GPU でコピーする。
    Mapped,
}

impl Upload {
    /// 1920x1080 の 1 フレームで両方を計測し速い方を返す。初めの数回はページフォールトで遅いので捨てる。
    pub fn measure(device: &wgpu::Device, queue: &wgpu::Queue) -> (Self, f64, f64) {
        const ROUNDS: usize = 24;
        const WARMUP: usize = 8;
        let format = Format {
            width: 1920,
            height: 1080,
            ..Format::default()
        };
        let mut pool = match Pool::new(device, format, 1, Shape::Planar) {
            Ok(pool) => pool,
            Err(_) => return (Self::Staged, f64::NAN, f64::NAN),
        };
        let frame = measured(format);
        let mut scratch = Vec::new();
        let mut time = |upload: Upload| {
            let mut taken = Vec::new();
            for round in 0..ROUNDS + WARMUP {
                let started = Instant::now();
                pool.write(device, queue, 0, &frame, upload, &mut scratch);
                // 空の送信でステージングを流し、両方とも GPU にデータが届くまでを計測する。
                queue.submit([]);
                device.poll(wgpu::PollType::wait_indefinitely()).ok();
                if round >= WARMUP {
                    taken.push(started.elapsed().as_secs_f64() * 1e3);
                }
            }
            taken.sort_by(f64::total_cmp);
            taken[taken.len() / 2]
        };
        let staged = time(Upload::Staged);
        let mapped = time(Upload::Mapped);
        let chosen = if mapped <= staged {
            Self::Mapped
        } else {
            Self::Staged
        };
        (chosen, staged, mapped)
    }
}

/// この PC で間に合う物。マシンによって負荷が大きく違うので必ず計測する。
#[derive(Debug, Clone)]
pub struct Capability {
    /// 候補の各方式で一フィールドにかかるミリ秒（計測した解像度で）。
    pub costs: Vec<(Deinterlace, f64)>,
    pub upload: Upload,
    pub staged_ms: f64,
    pub mapped_ms: f64,
}

impl Capability {
    pub fn measure(gpu: &Gpu, format: Format) -> Self {
        Self::measure_with(&gpu.device, &gpu.queue, format)
    }

    pub fn measure_with(device: &wgpu::Device, queue: &wgpu::Queue, format: Format) -> Self {
        const ROUNDS: usize = 30;
        const WARMUP: usize = 6;
        const GIVE_UP_AFTER: usize = 3;
        const GIVE_UP_OVER: f64 = 3.0;
        let (upload, staged_ms, mapped_ms) = Upload::measure(device, queue);
        // Weave は候補に無いが基準として計測する（他との差がフィルターそのものの負荷）。
        let mut costs = Vec::new();
        let mut video = match Video::for_measure(
            device,
            queue,
            wgpu::TextureFormat::Rgba8Unorm,
            format,
            Settings::default(),
        ) {
            Ok(video) => video,
            Err(_) => {
                return Self {
                    costs,
                    upload,
                    staged_ms,
                    mapped_ms,
                };
            }
        };
        let frame = measured(format);
        for quality in [
            Deinterlace::Weave,
            Deinterlace::Bwdif,
            Deinterlace::Yadif,
            Deinterlace::Bob,
        ] {
            video.set_settings(Settings {
                deinterlace: quality,
                ..video.settings()
            });
            video.reset();
            for _ in 0..3 {
                if video.push(&Decoded::Planes(frame.clone())).is_err() {
                    break;
                }
            }
            if !video.ready() {
                continue;
            }
            let mut taken = Vec::new();
            for round in 0..ROUNDS + WARMUP {
                let started = Instant::now();
                video.draw(None, false);
                device.poll(wgpu::PollType::wait_indefinitely()).ok();
                if round >= WARMUP {
                    taken.push(started.elapsed().as_secs_f64() * 1e3);
                }
                // 予算を大きく超える方式は、何回か測れば十分（遅い GPU では一回に 100 ms を超え、測り切ると数十秒かかる）。
                if taken.len() >= GIVE_UP_AFTER
                    && taken
                        .iter()
                        .all(|ms| *ms > FIELD_BUDGET_MS * AFFORDABLE * GIVE_UP_OVER)
                {
                    break;
                }
            }
            taken.sort_by(f64::total_cmp);
            costs.push((quality, taken[taken.len() / 2]));
        }
        Self {
            costs,
            upload,
            staged_ms,
            mapped_ms,
        }
    }

    /// 間に合う最良の方式。品質の順に辿って最初に収まる物を取る（負荷の順とは一致しない）。
    pub fn affordable(&self) -> Deinterlace {
        let budget = FIELD_BUDGET_MS * AFFORDABLE;
        for quality in Deinterlace::LADDER {
            if let Some((_, cost)) = self.costs.iter().find(|(step, _)| *step == quality)
                && *cost <= budget
            {
                return quality;
            }
        }
        Deinterlace::Bob
    }

    pub fn settings(&self, format: Format) -> Settings {
        let mut settings = Settings::of_format(format);
        if settings.deinterlace != Deinterlace::Weave {
            settings.deinterlace = self.affordable();
        }
        settings
    }
}

/// 計測に使うフレーム。平坦なフレームではフィルターの分岐が軽く出るので、斜めの縞と細かい縦の模様にする。
pub(super) fn measured(format: Format) -> Frame {
    let plane = |width: usize, height: usize| {
        let mut data = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                let band = ((x + y) / 12) % 2;
                data[y * width + x] = (40 + band * 120 + (x % 5) * 6 + (y % 3) * 9).min(235) as u8;
            }
        }
        Plane {
            data,
            stride: width,
            width,
            height,
        }
    };
    let (width, height) = (format.width as usize, format.height as usize);
    Frame {
        tag: 0,
        format,
        planes: [
            plane(width, height),
            plane(width / 2, height / 2),
            plane(width / 2, height / 2),
        ],
        structure: mediadecoder::es::Structure::Frame,
        top_field_first: true,
        repeat_first_field: false,
        progressive: false,
    }
}

/// マップしたままのバッファーの組の数。
const STAGING_SETS: usize = 3;

pub(super) fn padded(bytes: u32) -> u32 {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    bytes.div_ceil(align) * align
}

/// マップしたままのバッファー。順番が回ってきてもマップが戻っていなければステージング経由で送る。
#[derive(Debug)]
pub(super) struct Staging {
    pub(super) buffer: wgpu::Buffer,
    pub(super) ready: Arc<AtomicBool>,
}

impl Staging {
    pub(super) fn new(device: &wgpu::Device, size: u64) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("plane staging"),
                size,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            }),
            ready: Arc::new(AtomicBool::new(true)),
        }
    }

    pub(super) fn remap(&self) {
        let ready = self.ready.clone();
        self.buffer
            .map_async(wgpu::MapMode::Write, .., move |result| {
                ready.store(result.is_ok(), Ordering::Release);
            });
    }
}

/// スロットのデータの出どころ。
#[derive(Debug)]
pub(super) enum Store {
    Owned {
        planes: Vec<wgpu::Texture>,
    },
    /// 自前の物は無い。view はデコーダーが書いたメモリにバインドした物で、毎フレーム変わる。
    Imported,
}

#[derive(Debug)]
pub(super) struct Slot {
    pub(super) store: Store,
    pub(super) views: Vec<wgpu::TextureView>,
    /// 取り込んだフレームをスロットが使う間保持しておく物（無いと時間方向のフィルターが別のフレームを読む）。
    #[allow(dead_code)]
    pub(super) keep: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

/// 一つの形式のストリームを見せるテクスチャ。形式が変わったら（選局）作り直す。
#[derive(Debug)]
pub struct Pool {
    pub(super) slots: Vec<Slot>,
    /// マップしたままのバッファーの組（三プレーン分ずつ）。スロットごとには持たず、順に使い回す
    /// （GPU のコピーとマップのし直しが済むまでの数で足りる）。
    pub(super) staging: Vec<Vec<Staging>>,
    pub(super) turn: usize,
    pub(super) sizes: [(u32, u32); 3],
    pub(super) bytes: u32,
    pub(super) format: Format,
    pub(super) shape: Shape,
}

impl Pool {
    pub fn new(
        device: &wgpu::Device,
        format: Format,
        slots: usize,
        shape: Shape,
    ) -> Result<Self, Error> {
        if format.chroma != 1 {
            return Err(Error::Unsupported("only 4:2:0 is broadcast here"));
        }
        if format.width == 0 || format.height == 0 {
            return Err(Error::Unsupported("a picture of no size"));
        }
        let bytes = if format.depth > 8 { 2 } else { 1 };
        let texture_format = if format.depth > 8 {
            wgpu::TextureFormat::R16Uint
        } else {
            wgpu::TextureFormat::R8Unorm
        };
        let sizes = [
            (format.width, format.height),
            (format.width.div_ceil(2), format.height.div_ceil(2)),
            (format.width.div_ceil(2), format.height.div_ceil(2)),
        ];
        // ウィーブするフレーム（[`super::weave`]）は自前のテクスチャに書く。16 ビットの正規化形式はレンダーターゲットに
        // できないことがあるので、10 ビットは 32 ビットの浮動小数点で持つ。
        if shape == Shape::Biplanar && format.woven {
            let (luma, chroma) = if format.depth > 8 {
                (
                    wgpu::TextureFormat::R32Float,
                    wgpu::TextureFormat::Rg32Float,
                )
            } else {
                (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
            };
            let slots = (0..slots)
                .map(|_| {
                    let planes: Vec<wgpu::Texture> = [(sizes[0], luma), (sizes[1], chroma)]
                        .into_iter()
                        .map(|((width, height), format)| {
                            device.create_texture(&wgpu::TextureDescriptor {
                                label: Some("woven plane"),
                                size: wgpu::Extent3d {
                                    width,
                                    height,
                                    depth_or_array_layers: 1,
                                },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: wgpu::TextureDimension::D2,
                                format,
                                usage: wgpu::TextureUsages::TEXTURE_BINDING
                                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                                view_formats: &[],
                            })
                        })
                        .collect();
                    let views = planes
                        .iter()
                        .map(|plane| plane.create_view(&wgpu::TextureViewDescriptor::default()))
                        .collect();
                    Slot {
                        store: Store::Owned { planes },
                        views,
                        keep: None,
                    }
                })
                .collect();
            return Ok(Self {
                slots,
                staging: Vec::new(),
                turn: 0,
                sizes,
                bytes,
                format,
                shape,
            });
        }
        if shape == Shape::Biplanar {
            return Ok(Self {
                slots: (0..slots)
                    .map(|_| Slot {
                        store: Store::Imported,
                        views: Vec::new(),
                        keep: None,
                    })
                    .collect(),
                staging: Vec::new(),
                turn: 0,
                sizes,
                bytes,
                format,
                shape,
            });
        }
        let slots: Vec<Slot> = (0..slots)
            .map(|_| {
                let planes: Vec<wgpu::Texture> = (0..3)
                    .map(|plane| {
                        let (width, height) = sizes[plane];
                        device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("plane"),
                            size: wgpu::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: texture_format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        })
                    })
                    .collect();
                let views = planes
                    .iter()
                    .map(|plane| plane.create_view(&wgpu::TextureViewDescriptor::default()))
                    .collect();
                Slot {
                    store: Store::Owned { planes },
                    views,
                    keep: None,
                }
            })
            .collect();
        let staging = (0..STAGING_SETS.min(slots.len()))
            .map(|_| {
                (0..3)
                    .map(|plane| {
                        let (width, height) = sizes[plane];
                        Staging::new(device, u64::from(padded(width * bytes) * height))
                    })
                    .collect()
            })
            .collect();
        Ok(Self {
            slots,
            staging,
            turn: 0,
            sizes,
            bytes,
            format,
            shape,
        })
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub(super) fn adopt(
        &mut self,
        slot: usize,
        views: Vec<wgpu::TextureView>,
        keep: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    ) {
        if let Some(slot) = self.slots.get_mut(slot) {
            slot.views = views;
            slot.keep = keep;
        }
    }

    pub(super) fn owned(&self, slot: usize) -> Option<&[wgpu::Texture]> {
        match &self.slots.get(slot)?.store {
            Store::Owned { planes, .. } => Some(planes),
            Store::Imported => None,
        }
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 一フレームのプレーンを一つのスロットへ入れる。`Plane::stride` を行ごとに守る（行数も 1088 のことがある）。
    pub(super) fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: usize,
        frame: &Frame,
        upload: Upload,
        scratch: &mut Vec<u8>,
    ) {
        let Store::Owned { .. } = self.slots[slot].store else {
            return;
        };
        let set = (!self.staging.is_empty()).then(|| self.turn % self.staging.len());
        self.turn = self.turn.wrapping_add(1);
        let mut encoder = None;
        for plane in 0..3 {
            let (width, height) = self.sizes[plane];
            let row = width * self.bytes;
            let stride = padded(row);
            let source = &frame.planes[plane];
            let Store::Owned { planes } = &self.slots[slot].store else {
                return;
            };
            let staging = set.map(|set| &self.staging[set][plane]);
            let staging = staging.filter(|staging| {
                upload == Upload::Mapped && staging.ready.load(Ordering::Acquire)
            });
            if let Some(staging) = staging {
                if let Ok(mut view) = staging.buffer.get_mapped_range_mut(..) {
                    rows(source, row, stride, height, |at, bytes| {
                        view.slice(at..at + bytes.len()).copy_from_slice(bytes);
                    });
                } else {
                    staging.ready.store(false, Ordering::Release);
                    continue;
                }
                staging.buffer.unmap();
                staging.ready.store(false, Ordering::Release);
                let encoder = encoder.get_or_insert_with(|| {
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("upload"),
                    })
                });
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &staging.buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(stride),
                            rows_per_image: Some(height),
                        },
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &planes[plane],
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            } else {
                scratch.clear();
                scratch.resize((stride * height) as usize, 0);
                rows(source, row, stride, height, |at, bytes| {
                    scratch[at..at + bytes.len()].copy_from_slice(bytes);
                });
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &planes[plane],
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    scratch,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        if let Some(encoder) = encoder {
            queue.submit([encoder.finish()]);
            if let Some(set) = set {
                for staging in &self.staging[set] {
                    if !staging.ready.load(Ordering::Acquire) {
                        staging.remap();
                    }
                }
            }
        }
    }
}

/// プレーンを一行ずつ辿り、各行とコピー先の位置を渡す（マップしたバッファーは write-combining）。
pub(super) fn rows(
    plane: &Plane,
    row: u32,
    stride: u32,
    height: u32,
    mut put: impl FnMut(usize, &[u8]),
) {
    let blank = vec![0u8; row as usize];
    for y in 0..height as usize {
        let source = plane.row(y.min(plane.height.saturating_sub(1)));
        let at = y * stride as usize;
        let take = (row as usize).min(source.len());
        put(at, &source[..take]);
        // 足りない所は前の中身ではなく黒にする。
        if take < row as usize {
            put(at + take, &blank[take..]);
        }
    }
}
